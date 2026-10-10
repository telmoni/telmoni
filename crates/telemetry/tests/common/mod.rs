//! What the suites stand up around the module: ClickHouse as its two users —
//! the migrator, who applies the file and is not held by its row policy,
//! and the module's own, who is — and a span with nothing in it but what a
//! test sets.
#![allow(dead_code, reason = "each suite uses the part of the harness it needs")]

use chrono::{DateTime, SubsecRound, Utc};
use tokio::sync::OnceCell;
use uuid::Uuid;

use telmoni_shared::{ProjectId, SpanKind, SpanStatus};
use telmoni_telemetry::store::{self, SpanRow, Store};

/// ClickHouse as both of the users a tier runs it with.
pub(crate) struct ClickHouse {
    /// The migrator's user, whom `tenant_isolation` does not hold.
    pub(crate) migrator: clickhouse::Client,
    /// The module's user, through the one query module.
    pub(crate) store: Store,
    /// The one query module as the migrator's user: a server whose one user
    /// made the tables and reads them, as the Compose quickstart's does,
    /// where the policy holds nobody.
    pub(crate) unheld: Store,
    /// The module's user, bare: a query the query module would never send.
    pub(crate) telemetry: clickhouse::Client,
}

/// The file applied once per suite, as `telmoni migrate` applies it.
static MIGRATED: OnceCell<()> = OnceCell::const_new();

/// ClickHouse as `MIGRATOR_CLICKHOUSE_URL` and `TELEMETRY_CLICKHOUSE_URL` name
/// its two users, with the file applied; or `None`, with a notice, when
/// either is unset. `make test` and CI set both.
pub(crate) async fn clickhouse_or_skip(test: &str) -> Option<ClickHouse> {
    let url = |var: &str| std::env::var(var).ok().filter(|v| !v.trim().is_empty());
    let (Some(migrator), Some(telemetry)) = (
        url("MIGRATOR_CLICKHOUSE_URL"),
        url("TELEMETRY_CLICKHOUSE_URL"),
    ) else {
        eprintln!(
            "skipping {test}: MIGRATOR_CLICKHOUSE_URL or TELEMETRY_CLICKHOUSE_URL unset \
             (`make docker-up` starts a local ClickHouse; `make test` sets both)"
        );
        return None;
    };
    let unheld = Store::connect(&migrator).expect("MIGRATOR_CLICKHOUSE_URL is a ClickHouse URL");
    let migrator = store::client(&migrator).expect("MIGRATOR_CLICKHOUSE_URL is a ClickHouse URL");
    MIGRATED
        .get_or_init(|| async {
            telmoni_telemetry::schema::apply(&migrator)
                .await
                .expect("the ClickHouse file applies as the migrator");
        })
        .await;
    Some(ClickHouse {
        migrator,
        store: Store::connect(&telemetry).expect("TELEMETRY_CLICKHOUSE_URL is a ClickHouse URL"),
        unheld,
        telemetry: store::client(&telemetry).expect("TELEMETRY_CLICKHOUSE_URL is a ClickHouse URL"),
    })
}

/// A finished span of `kind`, received now, lasting `millis`, with nothing
/// else set.
pub(crate) fn span(project_id: &ProjectId, kind: SpanKind, name: &str, millis: i64) -> SpanRow {
    let now = Utc::now();
    received(project_id, kind, name, millis, now)
}

/// [`span`], received at `at`, to the microsecond the table keeps.
pub(crate) fn received(
    project_id: &ProjectId,
    kind: SpanKind,
    name: &str,
    millis: i64,
    at: DateTime<Utc>,
) -> SpanRow {
    let at = at.trunc_subsecs(6);
    SpanRow {
        project_id: project_id.clone(),
        received_at: at,
        units: 1,
        run_id: Uuid::new_v4(),
        agent: "nightly-report".into(),
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".into(),
        span_id: "00f067aa0ba902b7".into(),
        parent_span_id: String::new(),
        kind,
        name: name.into(),
        started_at: at - chrono::Duration::milliseconds(millis),
        ended_at: at,
        status: SpanStatus::Ok,
        error_type: String::new(),
        model: String::new(),
        provider: String::new(),
        input_tokens: 0,
        output_tokens: 0,
        cache_read_input_tokens: 0,
        cache_write_input_tokens: 0,
        customer_id: String::new(),
        conversation_id: String::new(),
        tags: Vec::new(),
        bound_stall_after_s: None,
        bound_max_steps: None,
        bound_max_cost_usd: None,
        sample_rate: None,
        attributes: Vec::new(),
    }
}
