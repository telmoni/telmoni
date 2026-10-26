//! The watchdog's heartbeats, as a module keeps them: a loop it runs is stale
//! until a replica beats it, fresh once any replica has, and stale again once
//! three of its periods pass unbeaten; a beat lands through the module's own
//! lane, in a table that names no tenant.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::time::Duration;

use sqlx::PgPool;

use common::AuthStub;
use telmoni_shared::heartbeat::Heartbeats;
use telmoni_telemetry::db::TelemetryLane;

/// The period a row holds for `name`.
async fn period_of(pool: &PgPool, name: &str) -> i32 {
    sqlx::query_scalar("SELECT period_seconds FROM telemetry.heartbeats WHERE name = $1")
        .bind(name)
        .fetch_one(pool)
        .await
        .expect("the loop's row")
}

/// ⚠ A loop stopped on every replica is named once three of its periods pass
/// unbeaten, and any replica's beat is the loop's.
#[sqlx::test]
async fn a_loop_is_stale_until_beaten_and_again_after_three_periods(pool: PgPool) {
    let state = common::state(&pool, AuthStub::new());
    assert!(
        state.stale().await.is_empty(),
        "a loop this replica does not run is never read"
    );

    state
        .heartbeats
        .runs("purges", Duration::from_secs(10 * 60));
    assert_eq!(state.stale().await, ["telemetry.purges"], "never beaten");

    state.heartbeats.beat(&state.db, "purges").await;
    assert!(state.stale().await.is_empty());
    assert_eq!(period_of(&pool, "purges").await, 600);

    sqlx::query("UPDATE telemetry.heartbeats SET beat_at = now() - interval '29 minutes'")
        .execute(&pool)
        .await
        .expect("an older beat");
    assert!(
        state.stale().await.is_empty(),
        "inside three periods of ten minutes"
    );
    sqlx::query("UPDATE telemetry.heartbeats SET beat_at = now() - interval '31 minutes'")
        .execute(&pool)
        .await
        .expect("a beat three periods old");
    assert_eq!(state.stale().await, ["telemetry.purges"]);

    let other = Heartbeats::new("telemetry", TelemetryLane);
    other.runs("purges", Duration::from_secs(10 * 60));
    other.beat(&state.db, "purges").await;
    assert!(
        state.stale().await.is_empty(),
        "another replica's beat is the loop's"
    );
}

/// A loop that ticks every few seconds is held to the watchdog's own minute,
/// and a replica writes its beat at most twice a minute.
#[sqlx::test]
async fn a_fast_loop_is_held_to_a_minute(pool: PgPool) {
    let state = common::state(&pool, AuthStub::new());
    state.heartbeats.runs("refusals", Duration::from_secs(5));
    state.heartbeats.beat(&state.db, "refusals").await;
    assert_eq!(period_of(&pool, "refusals").await, 60);

    sqlx::query("UPDATE telemetry.heartbeats SET beat_at = now() - interval '2 minutes'")
        .execute(&pool)
        .await
        .expect("an older beat");
    state.heartbeats.beat(&state.db, "refusals").await;
    let beaten: bool = sqlx::query_scalar(
        "SELECT beat_at > now() - interval '1 minute' FROM telemetry.heartbeats \
         WHERE name = 'refusals'",
    )
    .fetch_one(&pool)
    .await
    .expect("the loop's row");
    assert!(
        !beaten,
        "a beat within half a minute of the last is not written"
    );
    assert!(
        state.stale().await.is_empty(),
        "two minutes is inside three of a minute"
    );
}
