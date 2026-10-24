//! The data out a plan holds an organization to a day: counted where it is
//! sent bytes, refused once spent, and told to its owner and admins once a
//! day, in this process where Redis does not answer and over Redis as a tier
//! counts it.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use serde_json::Value;
use sqlx::PgPool;

use common::{AuthStub, DataOut, LoggedNotice, NoticeLog};
use telmoni_shared::seam::{Entitlements, NoLimits, Telemetry};
use telmoni_shared::{NotificationKind, OrganizationId, OrganizationStatus};
use telmoni_telemetry::AppState;
use telmoni_telemetry::ceilings;
use telmoni_telemetry::seam::TelemetrySeam;

/// Hobby's data out a day, in bytes.
const HOBBY: u64 = 5_000_000;

/// The module under `entitlements`, over no store, and the notices it hands
/// over.
fn module(
    pool: &PgPool,
    auth: Arc<AuthStub>,
    entitlements: Arc<dyn Entitlements>,
) -> (Arc<AppState>, Arc<NoticeLog>) {
    let notices = NoticeLog::new();
    let state = common::state_with_notices(pool, auth, entitlements, notices.clone());
    (state, notices)
}

/// The organization alerts `organization` was handed.
fn alerts(notices: &NoticeLog, organization: &OrganizationId) -> Vec<LoggedNotice> {
    notices
        .notices()
        .into_iter()
        .filter(|notice| {
            notice.kind == NotificationKind::OrganizationAlert
                && notice.organization_id == *organization
        })
        .collect()
}

/// The day's data out is refused once spent, a 429 until midnight UTC, and
/// the owner told once; a binary that sells nothing counts nothing.
#[sqlx::test]
async fn data_out_past_the_day_is_refused_until_midnight(pool: PgPool) {
    let organization = OrganizationId::new();
    let (state, notices) = module(&pool, AuthStub::new(), Arc::new(DataOut(HOBBY)));
    let seam = TelemetrySeam(state.clone());
    assert!(seam.admit_data_out(&organization).await.is_ok());
    seam.spend_data_out(&organization, HOBBY - 1).await;
    assert!(seam.admit_data_out(&organization).await.is_ok());
    seam.spend_data_out(&organization, 1).await;
    let problem = seam
        .admit_data_out(&organization)
        .await
        .expect_err("the day is spent")
        .to_problem_details();
    assert_eq!(problem.type_uri, "/errors/tenant/rate-limited");
    assert_eq!(problem.status, 429);
    let detail = problem.detail.unwrap_or_default();
    assert!(
        detail.contains("5 MB") && detail.contains("midnight UTC"),
        "{detail}"
    );
    assert!(
        problem
            .extensions
            .get("retry_after_secs")
            .and_then(Value::as_u64)
            .is_some_and(|s| (1..=86_400).contains(&s))
    );
    ceilings::tell(&state).await;
    ceilings::tell(&state).await;
    let told = alerts(&notices, &organization);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].metadata["lane"], "data_out");
    assert_eq!(told[0].metadata["refused"], 1);

    let (state, _) = module(&pool, AuthStub::new(), Arc::new(NoLimits));
    let seam = TelemetrySeam(state);
    seam.spend_data_out(&organization, u64::MAX).await;
    assert!(
        seam.admit_data_out(&organization).await.is_ok(),
        "nothing is counted without a plan"
    );
}

/// An organization being erased is told nothing of its refusals, its feed
/// about to be purged.
#[sqlx::test]
async fn an_organization_being_erased_is_told_nothing(pool: PgPool) {
    let organization = OrganizationId::new();
    let auth = AuthStub::new();
    auth.stand(&organization, Some(OrganizationStatus::PendingDeletion));
    let (state, notices) = module(&pool, auth, Arc::new(DataOut(HOBBY)));
    let seam = TelemetrySeam(state.clone());
    seam.spend_data_out(&organization, HOBBY).await;
    assert!(seam.admit_data_out(&organization).await.is_err());
    ceilings::tell(&state).await;
    assert!(alerts(&notices, &organization).is_empty());
}

/// Over Redis, as a tier counts it, a day spent on one replica is spent on
/// every one, and its notice is written once whichever replica tells it.
#[sqlx::test]
async fn the_ceiling_holds_over_redis_as_on_a_tier(pool: PgPool) {
    let organization = OrganizationId::new();
    let notices = NoticeLog::new();
    let replicas = [(); 2].map(|()| {
        common::state_with_redis_under(
            &pool,
            AuthStub::new(),
            Arc::new(DataOut(HOBBY)),
            notices.clone(),
        )
    });
    if replicas[0].redis.connection().await.is_none() {
        eprintln!(
            "skipping {}: Redis is not reachable (REDIS_URL, or the compose file's)",
            module_path!()
        );
        return;
    }
    TelemetrySeam(replicas[0].clone())
        .spend_data_out(&organization, HOBBY)
        .await;
    assert!(
        TelemetrySeam(replicas[1].clone())
            .admit_data_out(&organization)
            .await
            .is_err(),
        "the day spent on one replica is spent on the other"
    );
    for state in &replicas {
        ceilings::tell(state).await;
    }
    let told = alerts(&notices, &organization);
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].metadata["lane"], "data_out");
}
