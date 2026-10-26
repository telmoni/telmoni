//! What runs spent, read against a real ClickHouse with the prices Postgres
//! holds: each call's cost as reported, else its tokens at the price in force
//! the day it arrived, priced as the price book prices it, else named.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use common::{AuthStub, clickhouse_or_skip};
use telmoni_shared::{ProjectId, SpanKind};
use telmoni_telemetry::prices::{PriceBook, Usage};
use telmoni_telemetry::rules::RuleFilters;
use telmoni_telemetry::rules::evaluate::Window;
use telmoni_telemetry::spend;
use telmoni_telemetry::store::{Projects, SpanRow};

/// A call of `run` to `provider`'s `model`, received `at`.
fn call(
    project: &ProjectId,
    run: Uuid,
    (provider, model): (&str, &str),
    usage: Usage,
    cost: Option<f64>,
    at: DateTime<Utc>,
) -> SpanRow {
    SpanRow {
        run_id: run,
        provider: provider.into(),
        model: model.into(),
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cache_read_input_tokens: usage.cache_read_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        cost,
        ..common::received(
            project,
            SpanKind::ModelCall,
            &format!("chat {model}"),
            500,
            at,
        )
    }
}

fn usage(input: u64, output: u64, cache_read: u64, cache_write: u64) -> Usage {
    Usage {
        input_tokens: input,
        output_tokens: output,
        cache_read_input_tokens: cache_read,
        cache_write_input_tokens: cache_write,
    }
}

/// Prices as `migrate` writes them: a row for always, one from `today`, the
/// calls' day, that replaces it, one from the day after that does not yet,
/// and a name priced from the day after alone.
async fn price(pool: &PgPool, today: NaiveDate) {
    let tomorrow = today + TimeDelta::days(1);
    for (provider, model, from, input) in [
        ("openai", "gpt-4o", None, 2.5e-6),
        ("openai", "gpt-4o", Some(today), 2e-6),
        ("openai", "gpt-4o", Some(tomorrow), 1e-6),
        ("anthropic", "claude-sonnet-4-5", None, 3e-6),
        ("deepseek", "deepseek-chat", Some(tomorrow), 1e-6),
    ] {
        sqlx::query(
            "INSERT INTO telemetry.model_prices
                 (provider, model, effective_from, input, output, cache_read, cache_write)
             VALUES ($1, $2, $3, $4, $4 * 4, $4 / 10, $4 * 1.25)",
        )
        .bind(provider)
        .bind(model)
        .bind(from)
        .bind(input)
        .execute(pool)
        .await
        .expect("a price");
    }
}

/// ⚠ ClickHouse prices a call as the price book does: its input less the
/// cache's at the price in force by its day, each cache count at its own, a
/// cache past the input pricing no input; a reported cost, a free one among
/// them, is taken as reported; and a call with neither is named, once.
#[sqlx::test]
async fn a_run_is_priced_as_the_price_book_prices_it(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    // One instant, so no call lands on a day the prices were not dated by.
    let at = Utc::now() - TimeDelta::seconds(30);
    price(&pool, at.date_naive()).await;
    let state = common::state_with_stores(&pool, AuthStub::new());
    let project = ProjectId::new();
    let run = Uuid::new_v4();
    let gpt = ("openai", "gpt-4o");
    let claude = ("anthropic", "claude-sonnet-4-5");
    let priced = [
        (gpt, usage(1_000, 50, 200, 100)),
        (gpt, usage(100, 10, 300, 0)),
        (claude, usage(12_345, 678, 0, 9)),
    ];
    let mut rows: Vec<SpanRow> = priced
        .iter()
        .map(|(name, usage)| call(&project, run, *name, *usage, None, at))
        .collect();
    rows.extend([
        call(&project, run, gpt, usage(1_000, 1_000, 0, 0), Some(0.0), at),
        call(&project, run, gpt, usage(1, 1, 0, 0), Some(0.125), at),
        call(
            &project,
            run,
            ("openai", "ft:gpt-4o:acme"),
            usage(90, 9, 0, 0),
            None,
            at,
        ),
        call(
            &project,
            run,
            ("openai", "ft:gpt-4o:acme"),
            usage(1, 0, 0, 0),
            None,
            at,
        ),
        call(
            &project,
            run,
            ("deepseek", "deepseek-chat"),
            usage(7, 7, 0, 0),
            None,
            at,
        ),
        call(
            &project,
            run,
            ("ollama", "llama3.1:8b"),
            usage(0, 0, 0, 0),
            None,
            at,
        ),
    ]);
    ch.store.insert_spans(&rows).await.expect("the calls");

    let book = PriceBook::read(&pool).await.expect("the prices");
    let expected: f64 = 0.125
        + priced
            .iter()
            .map(|((provider, model), usage)| {
                book.price(provider, model, at)
                    .expect("a price in force")
                    .cost(*usage)
            })
            .sum::<f64>();
    let spent = spend::run(&state, &Projects::one(project.clone()), run)
        .await
        .expect("the run's spend");
    assert!(
        (spent.spent - expected).abs() < 1e-12,
        "{} against {expected}",
        spent.spent
    );
    assert_eq!(
        spent.unpriced,
        [
            ("deepseek".to_owned(), "deepseek-chat".to_owned()),
            ("openai".to_owned(), "ft:gpt-4o:acme".to_owned()),
        ],
        "a price not yet in force prices nothing, and a call with no tokens owes none"
    );
}

/// What each run spent over a window, folded: the window's calls alone,
/// those the filters admit, and the run that spent the most.
#[sqlx::test]
async fn the_runs_of_a_window_are_folded_by_what_each_spent(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let at = Utc::now() - TimeDelta::seconds(30);
    price(&pool, at.date_naive()).await;
    let state = common::state_with_stores(&pool, AuthStub::new());
    let project = ProjectId::new();
    let (dear, cheap, old) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let gpt = ("openai", "gpt-4o");
    ch.store
        .insert_spans(&[
            call(&project, dear, gpt, usage(0, 0, 0, 0), Some(3.0), at),
            call(&project, dear, gpt, usage(100_000, 20_000, 0, 0), None, at),
            call(&project, cheap, gpt, usage(0, 0, 0, 0), Some(0.5), at),
            SpanRow {
                agent: "other-agent".into(),
                ..call(&project, cheap, gpt, usage(0, 0, 0, 0), Some(9.0), at)
            },
            call(
                &project,
                old,
                gpt,
                usage(0, 0, 0, 0),
                Some(50.0),
                at - TimeDelta::hours(2),
            ),
        ])
        .await
        .expect("the calls");

    let window = Window::ending(Utc::now(), std::time::Duration::from_secs(3_600));
    let filters = RuleFilters {
        agent: vec!["nightly-report".into()],
        ..RuleFilters::default()
    };
    let read = spend::runs(&state, &Projects::one(project.clone()), window, &filters)
        .await
        .expect("the window's spend");
    // 100,000 in at their day's $2 a million and 20,000 out at $8 a million.
    let dear_spent = 3.0 + 0.2 + 0.16;
    assert_eq!(read.runs, 2, "the run older than the window is not read");
    assert!((read.most - dear_spent).abs() < 1e-12, "{read:?}");
    assert!((read.total - (dear_spent + 0.5)).abs() < 1e-12, "{read:?}");
    assert!(
        (read.mean - (dear_spent + 0.5) / 2.0).abs() < 1e-12,
        "{read:?}"
    );
    assert_eq!(read.costliest, dear);

    let nothing = spend::runs(
        &state,
        &Projects::one(ProjectId::new()),
        window,
        &RuleFilters::default(),
    )
    .await
    .expect("an empty window");
    assert_eq!(nothing.runs, 0);
}
