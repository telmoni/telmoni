//! What runs spent, read against a real ClickHouse with the prices Postgres
//! holds: each call's cost as reported, else its tokens at the price in force
//! the day it arrived, priced as the price book prices it, else named.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use chrono::{DateTime, Datelike as _, NaiveDate, TimeDelta, Timelike as _, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use common::{AuthStub, clickhouse_or_skip};
use telmoni_shared::{ProjectId, RuleWindow, SpanKind};
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

/// A project's own price comes before the table's, for its calls alone: a
/// fine-tune the table does not list is priced, and a listed model at the
/// project's price; another project's same calls keep the table's.
#[sqlx::test]
async fn a_projects_own_price_comes_before_the_tables(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let at = Utc::now() - TimeDelta::seconds(30);
    price(&pool, at.date_naive()).await;
    let state = common::state_with_stores(&pool, AuthStub::new());
    let (ours, theirs) = (ProjectId::new(), ProjectId::new());
    sqlx::query(
        "INSERT INTO telemetry.project_prices
             (project_id, position, provider, models, effective_from,
              input, output, cache_read, cache_write)
         VALUES ($1, 0, 'openai', ARRAY['gpt-4o', 'ft:gpt-4o:acme'], NULL, 1e-6, 2e-6, 0, 0)",
    )
    .bind(&ours)
    .execute(&pool)
    .await
    .expect("our price");
    let (our_run, their_run) = (Uuid::new_v4(), Uuid::new_v4());
    let calls = |project: &ProjectId, run: Uuid| {
        vec![
            call(
                project,
                run,
                ("openai", "gpt-4o"),
                usage(1_000, 100, 0, 0),
                None,
                at,
            ),
            call(
                project,
                run,
                ("openai", "ft:gpt-4o:acme"),
                usage(500, 50, 0, 0),
                None,
                at,
            ),
        ]
    };
    let rows: Vec<SpanRow> = calls(&ours, our_run)
        .into_iter()
        .chain(calls(&theirs, their_run))
        .collect();
    ch.store.insert_spans(&rows).await.expect("the calls");

    let mine = spend::run(&state, &Projects::one(ours.clone()), our_run)
        .await
        .expect("our run's spend");
    let expected = (1_000.0 + 500.0) * 1e-6 + (100.0 + 50.0) * 2e-6;
    assert!(
        (mine.spent - expected).abs() < 1e-12,
        "{} against {expected}",
        mine.spent
    );
    assert!(mine.unpriced.is_empty(), "{:?}", mine.unpriced);

    let book = PriceBook::read(&pool).await.expect("the prices");
    let table = book
        .price("openai", "gpt-4o", at)
        .expect("the table's")
        .cost(usage(1_000, 100, 0, 0));
    let yours = spend::run(&state, &Projects::one(theirs.clone()), their_run)
        .await
        .expect("their run's spend");
    assert!(
        (yours.spent - table).abs() < 1e-12,
        "{} against {table}",
        yours.spent
    );
    assert_eq!(
        yours.unpriced,
        [("openai".to_owned(), "ft:gpt-4o:acme".to_owned())],
        "our price is no other project's"
    );
}

/// A budget reads the month so far from its first at midnight UTC, scaled by each call's
/// sample rate; a tag takes the runs whose own span carries it.
#[sqlx::test]
async fn a_budget_reads_the_month_so_far_of_the_runs_its_tag_names(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let state = common::state_with_stores(&pool, AuthStub::new());
    let project = ProjectId::new();
    let at = Utc::now() - TimeDelta::seconds(30);
    let (team, other) = (Uuid::new_v4(), Uuid::new_v4());
    let gpt = ("openai", "gpt-4o");
    let tagged = |run: Uuid, team_tag: &str| SpanRow {
        run_id: run,
        tags: vec![("team".into(), team_tag.into())],
        ..common::received(&project, SpanKind::Run, "nightly-report", 100, at)
    };
    ch.store
        .insert_spans(&[
            call(&project, team, gpt, usage(0, 0, 0, 0), Some(2.0), at),
            call(&project, team, gpt, usage(0, 0, 0, 0), Some(1.5), at),
            tagged(team, "search"),
            // Kept one in two past the grant: the month's sum counts it twice.
            SpanRow {
                sample_rate: Some(0.5),
                ..call(&project, other, gpt, usage(0, 0, 0, 0), Some(7.0), at)
            },
            tagged(other, "billing"),
        ])
        .await
        .expect("the runs");

    let month = Window::of(Utc::now(), RuleWindow::Month);
    assert_eq!(
        (month.from.day(), month.from.hour(), month.from.minute()),
        (1, 0, 0),
        "the month from its first"
    );
    let filters = RuleFilters {
        tag: vec!["team:search".into()],
        ..RuleFilters::default()
    };
    let read = spend::runs(&state, &Projects::one(project.clone()), month, &filters)
        .await
        .expect("the month's spend");
    assert_eq!(read.runs, 1, "{read:?}");
    assert!((read.total - 3.5).abs() < 1e-12, "{read:?}");
    let all = spend::runs(
        &state,
        &Projects::one(project.clone()),
        month,
        &RuleFilters::default(),
    )
    .await
    .expect("the month's spend");
    assert!((all.total - 17.5).abs() < 1e-12, "{all:?}");
    assert!(
        (all.most - 7.0).abs() < 1e-12,
        "a run's own never scaled: {all:?}"
    );
}

/// ⚠ A sum over runs reads the full hours from the hourly totals, which
/// outlive the spans, and the rest from the spans, each call scaled by its
/// run's sample rate: the spans of an hour gone, its spend stands; the
/// filters hold on both; a run's own spend is never scaled.
#[sqlx::test]
async fn a_sum_over_runs_reads_the_hours_the_spans_no_longer_keep(pool: PgPool) {
    let Some(ch) = clickhouse_or_skip(module_path!()).await else {
        return;
    };
    let state = common::state_with_stores(&pool, AuthStub::new());
    let project = ProjectId::new();
    let (sampled, whole) = (Uuid::new_v4(), Uuid::new_v4());
    let gpt = ("openai", "gpt-4o");
    let now = Utc::now();
    let earlier = now - TimeDelta::hours(3);
    let at = now - TimeDelta::seconds(30);
    let rated = |row: SpanRow, rate: f64| SpanRow {
        sample_rate: Some(rate),
        ..row
    };
    ch.store
        .insert_spans(&[
            // One run in four sent: it stands for four.
            rated(
                call(
                    &project,
                    sampled,
                    gpt,
                    usage(0, 0, 0, 0),
                    Some(2.0),
                    earlier,
                ),
                0.25,
            ),
            rated(
                call(&project, sampled, gpt, usage(0, 0, 0, 0), Some(1.0), at),
                0.25,
            ),
            call(&project, whole, gpt, usage(0, 0, 0, 0), Some(0.5), at),
            SpanRow {
                agent: "other-agent".into(),
                ..call(&project, whole, gpt, usage(0, 0, 0, 0), Some(9.0), earlier)
            },
        ])
        .await
        .expect("the calls");
    let projects = Projects::one(project.clone());
    let window = Window::between(now - TimeDelta::hours(6), now);
    let filters = RuleFilters {
        agent: vec!["nightly-report".into()],
        ..RuleFilters::default()
    };
    let expected = 2.0 * 4.0 + 1.0 * 4.0 + 0.5;
    let summed = spend::summed(&state, &projects, window, &filters)
        .await
        .expect("the window's sum")
        .expect("calls landed");
    assert!((summed - expected).abs() < 1e-12, "{summed}");
    let runs = spend::runs(&state, &projects, window, &filters)
        .await
        .expect("each run's spend");
    assert!(
        (runs.total - expected).abs() < 1e-12,
        "both reads agree: {runs:?}"
    );
    assert!(
        (runs.most - 3.0).abs() < 1e-12,
        "a run's own never scaled: {runs:?}"
    );

    // The hour's spans purged, as a short retention purges them: the totals keep it.
    ch.migrator
        .query(
            "DELETE FROM telemetry.spans WHERE project_id = ? \
             AND received_at < fromUnixTimestamp64Micro(?)",
        )
        .bind(project.as_str())
        .bind((earlier + TimeDelta::minutes(1)).timestamp_micros())
        .execute()
        .await
        .expect("the earlier spans purged");
    let after = spend::summed(&state, &projects, window, &filters)
        .await
        .expect("the window's sum")
        .expect("calls landed");
    assert!((after - expected).abs() < 1e-12, "{after}");
    let runs = spend::runs(&state, &projects, window, &filters)
        .await
        .expect("each run's spend");
    assert!(
        (runs.total - (1.0 * 4.0 + 0.5)).abs() < 1e-12,
        "the spans hold what is left: {runs:?}"
    );

    let empty = spend::summed(
        &state,
        &Projects::one(ProjectId::new()),
        window,
        &RuleFilters::default(),
    )
    .await
    .expect("an empty window");
    assert_eq!(empty, None, "no call, no sum");
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
