//! An agent's versions side by side, against a real ClickHouse: a release
//! failing twice as often and scoring lower reads so beside the one before
//! it; a version is measured on its ended runs alone, and the runs naming
//! none are a row of their own, last; a run counts under each value its
//! spans in the environment asked carry, and is measured whole; a prompt's
//! versions are read by name and version apart; a score is read once a run
//! and version; the window is held to what the plan and the project keep;
//! and the lane reads any agent's name, for the project's readers alone.
#![expect(clippy::expect_used, reason = "test scaffolding")]

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Duration, DurationRound, SubsecRound, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use common::{AuthStub, ClickHouse, SECRET, clickhouse_or_skip, state_with_stores_under};
use telmoni_shared::middleware::service_auth::SERVICE_SECRET_HEADER;
use telmoni_shared::seam::{Entitlements, NoLimits};
use telmoni_shared::{
    Grants, OrganizationId, OrganizationRole, ProjectId, Role, ScoreSource, ScoreTarget, ScoreType,
    SpanKind, SpanStatus, TelmoniError,
};
use telmoni_telemetry::router;
use telmoni_telemetry::scores::ScoreRow;
use telmoni_telemetry::store::SpanRow;

const LANE: &str = "/internal/v1/telemetry/agent-versions";
const AGENT: &str = "support";
const MEMBER: &str = "user_member";
const STRANGER: &str = "user_stranger";
const THEIR_OWNER: &str = "user_other";
const PRODUCTION: (&str, &str) = ("production", "");

/// Two projects over both stores: ours, a member seated on it, and
/// another's.
struct Lane {
    ch: ClickHouse,
    app: Router,
    organization: OrganizationId,
    ours: ProjectId,
    theirs: ProjectId,
}

async fn lane(pool: &PgPool, test: &str) -> Option<Lane> {
    lane_under(pool, test, Arc::new(NoLimits)).await
}

async fn lane_under(
    pool: &PgPool,
    test: &str,
    entitlements: Arc<dyn Entitlements>,
) -> Option<Lane> {
    let ch = clickhouse_or_skip(test).await?;
    let (organization, ours, theirs) = (OrganizationId::new(), ProjectId::new(), ProjectId::new());
    let auth = AuthStub::new();
    auth.seat(
        MEMBER,
        &organization,
        &ours,
        OrganizationRole::Member,
        Some(Role::Member),
    );
    auth.seat(
        THEIR_OWNER,
        &OrganizationId::new(),
        &theirs,
        OrganizationRole::Owner,
        None,
    );
    Some(Lane {
        ch,
        app: router(state_with_stores_under(pool, auth, entitlements)),
        organization,
        ours,
        theirs,
    })
}

/// The lane's path with `query`, each value encoded as a browser sends it.
fn path(query: &[(&str, &str)]) -> String {
    let url = reqwest::Url::parse_with_params("http://server/", query).expect("a URL");
    format!("{LANE}?{}", url.query().unwrap_or_default())
}

impl Lane {
    async fn ask(&self, user: &str, project: &ProjectId, path: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("GET")
            .uri(path)
            .header(SERVICE_SECRET_HEADER, SECRET)
            .header("authorization", format!("Bearer tok_{user}"))
            .header("x-organization-id", self.organization.as_str())
            .header("x-project-id", project.as_str())
            .body(Body::empty())
            .expect("a request");
        let response = self
            .app
            .clone()
            .oneshot(request)
            .await
            .expect("the router answers");
        let status = response.status();
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("a body")
            .to_bytes();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("a JSON body")
        };
        (status, body)
    }

    /// The lane as `MEMBER` reads it on our project.
    async fn read(&self, query: &[(&str, &str)]) -> Value {
        let path = path(query);
        let (status, body) = self.ask(MEMBER, &self.ours, &path).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
        body
    }

    /// What the lane answers `MEMBER` a query it may refuse.
    async fn status(&self, query: &str) -> StatusCode {
        self.ask(MEMBER, &self.ours, &format!("{LANE}{query}"))
            .await
            .0
    }

    async fn keep(&self, spans: &[SpanRow]) {
        self.ch.store.insert_spans(spans).await.expect("kept");
    }

    async fn score(&self, scores: &[ScoreRow]) {
        self.ch
            .store
            .insert_scores_once(scores, &Uuid::new_v4().to_string())
            .await
            .expect("the scores are kept");
    }
}

/// One run being written: every span of it shares its id and its agent, and
/// is received a second after the run started.
struct Run<'a> {
    project: &'a ProjectId,
    id: Uuid,
    agent: &'a str,
    started: DateTime<Utc>,
}

/// A run of `agent` that started `minutes` ago.
fn run<'a>(project: &'a ProjectId, agent: &'a str, minutes: i64) -> Run<'a> {
    Run {
        project,
        id: Uuid::new_v4(),
        agent,
        started: Utc::now().trunc_subsecs(6) - Duration::minutes(minutes),
    }
}

impl Run<'_> {
    /// A span of the run sent from `from`, an environment and a release,
    /// starting `after` milliseconds into it and lasting `millis`.
    fn span(&self, kind: SpanKind, from: (&str, &str), after: i64, millis: i64) -> SpanRow {
        let started = self.started + Duration::milliseconds(after);
        SpanRow {
            run_id: self.id,
            agent: self.agent.into(),
            span_id: format!("{:016x}", Uuid::new_v4().as_u64_pair().0),
            environment: from.0.into(),
            release: from.1.into(),
            started_at: started,
            ended_at: started + Duration::milliseconds(millis),
            ..common::received(
                self.project,
                kind,
                "a span",
                millis,
                self.started + Duration::seconds(1),
            )
        }
    }

    /// The run's own span, which ends it: `millis` long, failed or not.
    fn own(&self, from: (&str, &str), millis: i64, failed: bool) -> SpanRow {
        SpanRow {
            status: if failed {
                SpanStatus::Error
            } else {
                SpanStatus::Ok
            },
            ..self.span(SpanKind::Run, from, 0, millis)
        }
    }

    /// A call inside the run answered by `model`, reporting `cost`.
    fn call(&self, from: (&str, &str), model: &str, cost: f64) -> SpanRow {
        SpanRow {
            model: model.into(),
            cost: Some(cost),
            ..self.span(SpanKind::ModelCall, from, 10, 50)
        }
    }

    /// A step inside the run carrying `attributes`.
    fn step(&self, from: (&str, &str), attributes: &[(&str, &str)]) -> SpanRow {
        SpanRow {
            attributes: attributes
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
            ..self.span(SpanKind::Step, from, 5, 20)
        }
    }

    /// An ended run of one release: its own span and one call.
    fn ended(&self, from: (&str, &str), millis: i64, cost: f64, failed: bool) -> Vec<SpanRow> {
        vec![
            self.own(from, millis, failed),
            self.call(from, "gpt-4o-mini", cost),
        ]
    }

    /// A number given the run under `name`, now.
    fn scored(&self, name: &str, value: f64) -> ScoreRow {
        ScoreRow {
            project_id: self.project.clone(),
            received_at: Utc::now().trunc_subsecs(6),
            score_id: Uuid::new_v4(),
            target: ScoreTarget::Run,
            run_id: self.id,
            trace_id: String::new(),
            span_id: String::new(),
            session_id: String::new(),
            name: name.into(),
            data_type: ScoreType::Number,
            value,
            categories: Vec::new(),
            range_min: None,
            range_max: None,
            source: ScoreSource::Sdk,
            scorer: String::new(),
            scorer_version: String::new(),
            is_deleted: 0,
        }
    }
}

/// A step's attributes naming a prompt and its version.
fn prompted<'a>(name: &'a str, version: &'a str) -> [(&'a str, &'a str); 2] {
    [
        ("gen_ai.prompt.name", name),
        ("gen_ai.prompt.version", version),
    ]
}

fn rows(answer: &Value) -> &Vec<Value> {
    answer["versions"].as_array().expect("versions")
}

/// The answer's values, in its order.
fn values(answer: &Value) -> Vec<&str> {
    rows(answer)
        .iter()
        .map(|row| row["value"].as_str().expect("a value"))
        .collect()
}

/// The answer's row of `value`.
fn row<'a>(answer: &'a Value, value: &str) -> &'a Value {
    rows(answer)
        .iter()
        .find(|row| row["value"] == value)
        .expect("a row of that value")
}

fn first_at(row: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(row["first_at"].as_str().expect("a time"))
        .expect("an RFC 3339 time")
        .with_timezone(&Utc)
}

/// ⚠ The page's question, *did the new release break it*: the newer release
/// first, failing twice as often and scoring lower, each on its own runs,
/// and nothing of another agent's or another project's among them.
#[sqlx::test]
async fn a_release_failing_twice_as_often_and_scoring_lower_reads_so(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let (before, after) = (("production", "2026.11.2"), ("production", "2026.11.3"));
    let (mut spans, mut scores) = (Vec::new(), Vec::new());
    let mut oldest = None;
    for n in 0..10 {
        let old = run(&l.ours, AGENT, 120 - n);
        spans.extend(old.ended(before, 1_000, 0.02, n < 2));
        if n == 0 {
            oldest = Some(old.started);
        }
        if n < 5 {
            scores.push(old.scored("helpful", 0.75));
        }
        let new = run(&l.ours, AGENT, 60 - n);
        spans.extend(new.ended(after, 2_000, 0.04, n < 4));
        if let Some(value) = [0.5, 0.25, 0.75].get(usize::try_from(n).expect("a count")) {
            scores.push(new.scored("helpful", *value));
        }
    }
    // The same release on another agent, and on another project's agent of
    // the same name: neither is this agent's.
    for n in 0..3 {
        let other = run(&l.ours, "nightly-report", 30 - n);
        spans.extend(other.ended(after, 9_000, 9.0, true));
        scores.push(other.scored("helpful", 0.0));
        let theirs = run(&l.theirs, AGENT, 30 - n);
        spans.extend(theirs.ended(after, 9_000, 9.0, true));
        scores.push(theirs.scored("helpful", 0.0));
    }
    l.keep(&spans).await;
    l.score(&scores).await;

    let answer = l.read(&[("agent", AGENT)]).await;
    assert_eq!(answer["days"], 14);
    assert_eq!(answer["by"], "release", "releases where none is asked");
    assert_eq!(answer["environment"], "production");
    assert_eq!(
        answer["environments"],
        json!([{ "value": "production", "runs": 20 }])
    );
    assert_eq!(answer["prompt"], Value::Null);
    assert_eq!(answer["prompts"], json!([]));
    assert_eq!(answer["scores"], json!(["helpful"]));
    assert_eq!(answer["more"], false);
    assert_eq!(
        values(&answer),
        ["2026.11.3", "2026.11.2"],
        "the newest first"
    );
    let (new, old) = (row(&answer, "2026.11.3"), row(&answer, "2026.11.2"));
    assert_eq!((&old["runs"], &old["failed_runs"]), (&json!(10), &json!(2)));
    assert_eq!(
        (&new["runs"], &new["failed_runs"]),
        (&json!(10), &json!(4)),
        "twice as often"
    );
    assert_eq!(
        old["scores"],
        json!([{ "name": "helpful", "mean": 0.75, "count": 5 }])
    );
    assert_eq!(
        new["scores"],
        json!([{ "name": "helpful", "mean": 0.5, "count": 3 }]),
        "and scoring lower"
    );
    assert_eq!(
        (&old["p50_ms"], &old["p95_ms"]),
        (&json!(1_000), &json!(1_000))
    );
    assert_eq!(
        (&new["p50_ms"], &new["p95_ms"]),
        (&json!(2_000), &json!(2_000))
    );
    assert_eq!(old["median_spend"], 0.02);
    assert_eq!(new["median_spend"], 0.04);
    assert_eq!(
        Some(first_at(old)),
        oldest,
        "when the first of its runs started"
    );
}

/// A version is measured on its ended runs: one whose runs are all still
/// running has no duration and no spend, never a zero, while its failures
/// count; a run that cost nothing costs zero; and the runs naming no
/// release are a row of their own, last however new.
#[sqlx::test]
async fn a_version_is_measured_on_its_ended_runs_and_the_runs_naming_none_come_last(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let a = ("production", "a");
    let mut spans = Vec::new();
    for (minutes, millis, cost) in [(60, 100, 0.10), (59, 200, 0.30), (58, 900, 0.20)] {
        spans.extend(run(&l.ours, AGENT, minutes).ended(a, millis, cost, false));
    }
    // Still running: a call of it has landed, its own span has not.
    spans.push(run(&l.ours, AGENT, 57).call(a, "gpt-4o-mini", 5.0));
    l.keep(&spans).await;

    let alone = l.read(&[("agent", AGENT), ("by", "release")]).await;
    assert_eq!(values(&alone), ["a"], "one version, with none before it");
    let only = row(&alone, "a");
    assert_eq!(
        (&only["runs"], &only["failed_runs"]),
        (&json!(4), &json!(0))
    );
    assert_eq!(
        (&only["p50_ms"], &only["p95_ms"]),
        (&json!(200), &json!(900)),
        "of the three that ended"
    );
    assert_eq!(
        only["median_spend"], 0.2,
        "the median of 0.10, 0.30 and 0.20"
    );
    assert_eq!(only["scores"], json!([]));

    let (b, c) = (("production", "b"), ("production", "c"));
    let failing = run(&l.ours, AGENT, 29);
    let free = run(&l.ours, AGENT, 20);
    let unnamed = run(&l.ours, AGENT, 5);
    l.keep(&[
        run(&l.ours, AGENT, 30).call(b, "gpt-4o-mini", 1.0),
        SpanRow {
            status: SpanStatus::Error,
            ..failing.call(b, "gpt-4o-mini", 1.0)
        },
        free.own(c, 1_500, false),
        unnamed.own(PRODUCTION, 700, false),
    ])
    .await;

    let answer = l.read(&[("agent", AGENT), ("by", "release")]).await;
    assert_eq!(
        values(&answer),
        ["c", "b", "a", ""],
        "the newest first seen first, the runs naming none last"
    );
    let running = row(&answer, "b");
    assert_eq!(
        (&running["runs"], &running["failed_runs"]),
        (&json!(2), &json!(1)),
        "a failed span fails a run still running"
    );
    for measure in ["p50_ms", "p95_ms", "median_spend"] {
        assert_eq!(
            running[measure],
            Value::Null,
            "{measure}: no run of it has ended"
        );
    }
    let free = row(&answer, "c");
    assert_eq!(free["p50_ms"], 1_500);
    assert_eq!(
        free["median_spend"], 0.0,
        "nothing spent is zero, not unknown"
    );
    let unnamed = row(&answer, "");
    assert_eq!(
        (&unnamed["runs"], &unnamed["p50_ms"]),
        (&json!(1), &json!(700))
    );
    assert_eq!(answer["more"], false);
}

/// By model, a run counts under each model one of its calls answered with,
/// and one that called none under no model; a model named on a span that is
/// no call is not one.
#[sqlx::test]
async fn a_run_counts_under_each_model_it_called(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let (both, one, none) = (
        run(&l.ours, AGENT, 30),
        run(&l.ours, AGENT, 20),
        run(&l.ours, AGENT, 10),
    );
    l.keep(&[
        both.own(PRODUCTION, 1_000, false),
        both.call(PRODUCTION, "gpt-4o-mini", 0.01),
        both.call(PRODUCTION, "claude-sonnet-5-5", 0.02),
        one.own(PRODUCTION, 1_000, true),
        one.call(PRODUCTION, "gpt-4o-mini", 0.01),
        SpanRow {
            model: "named-on-the-run".into(),
            ..none.own(PRODUCTION, 1_000, false)
        },
    ])
    .await;

    let answer = l.read(&[("agent", AGENT), ("by", "model")]).await;
    assert_eq!(answer["by"], "model");
    assert_eq!(
        values(&answer),
        ["claude-sonnet-5-5", "gpt-4o-mini", ""],
        "first seen in one run, so by name"
    );
    let mini = row(&answer, "gpt-4o-mini");
    assert_eq!(
        (&mini["runs"], &mini["failed_runs"]),
        (&json!(2), &json!(1))
    );
    assert_eq!(row(&answer, "claude-sonnet-5-5")["runs"], 1);
    assert_eq!(row(&answer, "")["runs"], 1, "the run that called none");
}

/// ⚠ A run whose spans were sent from two environments counts, in each, by
/// what its spans there carry — another environment's release, model or
/// prompt is never set beside this one's — and is measured whole; the
/// environment most runs were sent from is read where none is asked.
#[sqlx::test]
async fn a_run_crossing_environments_counts_by_the_spans_of_the_one_asked(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let (staging, production, unnamed) = (("staging", "A"), ("production", "B"), ("", "C"));
    let crossing = run(&l.ours, AGENT, 30);
    let mut spans = vec![
        crossing.own(production, 5_000, false),
        crossing.call(staging, "model-staging", 0.25),
        crossing.step(staging, &prompted("triage", "v1")),
        crossing.call(production, "model-production", 0.5),
        crossing.step(production, &prompted("triage", "v2")),
        crossing.span(SpanKind::ToolCall, unnamed, 20, 10),
    ];
    for minutes in [20, 10] {
        let within = run(&l.ours, AGENT, minutes);
        spans.push(within.own(production, 1_000, false));
        spans.push(within.call(production, "model-production", 0.5));
    }
    let staged = run(&l.ours, AGENT, 40);
    spans.push(staged.own(("staging", "S"), 1_000, false));
    l.keep(&spans).await;

    let answer = l.read(&[("agent", AGENT)]).await;
    assert_eq!(answer["environment"], "production", "where most runs are");
    assert_eq!(
        answer["environments"],
        json!([
            { "value": "production", "runs": 3 },
            { "value": "staging", "runs": 2 },
            { "value": "", "runs": 1 },
        ])
    );
    assert_eq!(values(&answer), ["B"], "no other environment's release");
    let b = row(&answer, "B");
    assert_eq!(b["runs"], 3);
    assert_eq!(b["p95_ms"], 5_000, "the crossing run, whole");
    assert_eq!(b["median_spend"], 0.5);

    let read = |environment: &'static str, by: &'static str| {
        let l = &l;
        async move {
            l.read(&[("agent", AGENT), ("by", by), ("environment", environment)])
                .await
        }
    };
    let in_staging = read("staging", "release").await;
    assert_eq!(in_staging["environment"], "staging");
    assert_eq!(values(&in_staging), ["A", "S"]);
    assert_eq!(
        row(&in_staging, "A")["p50_ms"],
        5_000,
        "measured whole, its own span in production among it"
    );
    assert_eq!(
        row(&in_staging, "A")["median_spend"],
        0.75,
        "and spending all it spent"
    );
    let in_none = read("", "release").await;
    assert_eq!(in_none["environment"], "");
    assert_eq!(values(&in_none), ["C"], "the spans naming no environment");

    assert_eq!(
        values(&read("production", "model").await),
        ["model-production"]
    );
    assert_eq!(
        values(&read("staging", "model").await),
        ["model-staging", ""],
        "and the staging run that called none"
    );
    let prompts = read("production", "prompt").await;
    assert_eq!(prompts["prompt"], "triage");
    assert_eq!(
        prompts["prompts"],
        json!([{ "value": "triage", "runs": 1 }])
    );
    assert_eq!(values(&prompts), ["v2"]);
    assert_eq!(values(&read("staging", "prompt").await), ["v1"]);
    let elsewhere = read("canary", "release").await;
    assert_eq!(elsewhere["environment"], "canary");
    assert_eq!(
        values(&elsewhere),
        [] as [&str; 0],
        "no run was sent from it"
    );
}

/// ⚠ A prompt's versions are those on the spans naming it, name and version
/// read apart: `triage:blue` at `v2` and `triage` at `blue:v2` are two
/// prompts, a span naming it with no version is its row with none, another
/// prompt's version is never its own, and a run that never named it is in
/// no row.
#[sqlx::test]
async fn a_prompts_versions_are_read_by_name_and_version_apart(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let named = |minutes: i64, prompts: &[(&str, &str)]| {
        let run = run(&l.ours, AGENT, minutes);
        let mut spans = vec![run.own(PRODUCTION, 1_000, false)];
        for (name, version) in prompts {
            spans.push(if version.is_empty() {
                run.step(PRODUCTION, &[("gen_ai.prompt.name", *name)])
            } else {
                run.step(PRODUCTION, &prompted(name, version))
            });
        }
        spans
    };
    let long = "é".repeat(1_024);
    let spans = [
        named(70, &[("triage:blue", "v2")]),
        named(60, &[("triage", "blue:v2")]),
        named(50, &[("triage:blue", "v2"), ("triage", "blue:v2")]),
        named(40, &[("triage", "")]),
        named(30, &[("triage", "3"), ("other", "9")]),
        named(20, &[("分類-é", "β")]),
        named(15, &[(long.as_str(), "1")]),
        named(10, &[]),
    ]
    .concat();
    l.keep(&spans).await;
    let of = |prompt: &'static str| {
        let l = &l;
        async move {
            l.read(&[("agent", AGENT), ("by", "prompt"), ("prompt", prompt)])
                .await
        }
    };

    let default = l.read(&[("agent", AGENT), ("by", "prompt")]).await;
    assert_eq!(default["by"], "prompt");
    assert_eq!(default["prompt"], "triage", "the prompt most runs name");
    assert_eq!(
        default["prompts"],
        json!([
            { "value": "triage", "runs": 4 },
            { "value": "triage:blue", "runs": 2 },
            { "value": "other", "runs": 1 },
            { "value": long, "runs": 1 },
            { "value": "分類-é", "runs": 1 },
        ])
    );
    assert_eq!(
        values(&default),
        ["3", "blue:v2", ""],
        "never `v2`, another prompt's, nor `9`, another span's"
    );
    assert_eq!(row(&default, "blue:v2")["runs"], 2);
    assert_eq!(row(&default, "3")["runs"], 1);
    assert_eq!(row(&default, "")["runs"], 1, "named with no version");
    assert_eq!(default, of("triage").await, "asked for by name, the same");

    let blue = of("triage:blue").await;
    assert_eq!(blue["prompt"], "triage:blue");
    assert_eq!(values(&blue), ["v2"]);
    assert_eq!(row(&blue, "v2")["runs"], 2);
    assert_eq!(values(&of("分類-é").await), ["β"]);
    let longest = l
        .read(&[("agent", AGENT), ("by", "prompt"), ("prompt", &long)])
        .await;
    assert_eq!(values(&longest), ["1"], "a name as long as an attribute");

    let unknown = of("never-named").await;
    assert_eq!(unknown["prompt"], "never-named");
    assert_eq!(values(&unknown), [] as [&str; 0]);
    for (length, status) in [
        (128, StatusCode::OK),
        (129, StatusCode::OK),
        (1_024, StatusCode::OK),
        (1_025, StatusCode::BAD_REQUEST),
    ] {
        let prompt = "p".repeat(length);
        let path = path(&[("agent", AGENT), ("by", "prompt"), ("prompt", &prompt)]);
        let (answered, _) = l.ask(MEMBER, &l.ours, &path).await;
        assert_eq!(answered, status, "a prompt of {length} characters");
    }

    let unprompted = l
        .read(&[("agent", "nightly-report"), ("by", "prompt")])
        .await;
    assert_eq!(unprompted["prompt"], Value::Null, "it names no prompt");
    assert_eq!(unprompted["prompts"], json!([]));
    assert_eq!(values(&unprompted), [] as [&str; 0]);
}

/// ⚠ A score is read once a run and version, however many of the run's
/// spans carry the version, and a run of two versions gives its score to
/// each; a score taken back, a category, one given before the window, a
/// session's and another project's are no part of a mean, and the rows show
/// the five scores given most.
#[sqlx::test]
async fn a_score_is_read_once_a_run_and_version(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let (r1, r2, r3) = (
        ("production", "r1"),
        ("production", "r2"),
        ("production", "r3"),
    );
    let (a, b, c, d) = (
        run(&l.ours, AGENT, 60),
        run(&l.ours, AGENT, 50),
        run(&l.ours, AGENT, 40),
        run(&l.ours, AGENT, 30),
    );
    l.keep(&[
        a.own(r1, 1_000, false),
        a.call(r1, "gpt-4o-mini", 0.01),
        a.step(r1, &[]),
        b.own(r1, 1_000, false),
        c.own(r2, 1_000, false),
        d.own(r1, 1_000, false),
        d.step(r3, &[]),
    ])
    .await;
    let now = Utc::now().trunc_subsecs(6);
    let taken_back = Uuid::new_v4();
    let mut scores = vec![
        a.scored("helpful", 1.0),
        // One of its spans', which names its run too.
        ScoreRow {
            target: ScoreTarget::Span,
            span_id: "00f067aa0ba902b7".into(),
            ..a.scored("helpful", 0.5)
        },
        ScoreRow {
            data_type: ScoreType::Boolean,
            ..a.scored("passed", 1.0)
        },
        b.scored("helpful", 0.0),
        d.scored("helpful", 0.5),
        c.scored("zz", 1.0),
        // None of these is part of a mean.
        ScoreRow {
            data_type: ScoreType::Category,
            categories: vec!["polite".into()],
            ..a.scored("tone", 0.0)
        },
        ScoreRow {
            score_id: taken_back,
            received_at: now - Duration::minutes(5),
            ..a.scored("helpful", 1.0)
        },
        ScoreRow {
            score_id: taken_back,
            is_deleted: 1,
            ..a.scored("helpful", 1.0)
        },
        ScoreRow {
            received_at: now - Duration::days(15),
            ..a.scored("helpful", 1.0)
        },
        ScoreRow {
            target: ScoreTarget::Session,
            run_id: Uuid::nil(),
            session_id: "s_1".into(),
            ..a.scored("helpful", 1.0)
        },
        ScoreRow {
            project_id: l.theirs.clone(),
            ..a.scored("helpful", 1.0)
        },
    ];
    for name in ["a", "b", "c"] {
        scores.push(c.scored(name, 0.25));
        scores.push(c.scored(name, 0.75));
    }
    l.score(&scores).await;

    let answer = l.read(&[("agent", AGENT)]).await;
    assert_eq!(
        answer["scores"],
        json!(["helpful", "a", "b", "c", "passed"]),
        "the five given most, then by name: `zz` is the sixth"
    );
    assert_eq!(values(&answer), ["r3", "r2", "r1"]);
    assert_eq!(
        row(&answer, "r1")["scores"],
        json!([
            { "name": "helpful", "mean": 0.5, "count": 4 },
            { "name": "passed", "mean": 1.0, "count": 1 },
        ]),
        "1, 0.5, 0 and 0.5, each once though three of a run's spans carry r1"
    );
    assert_eq!(
        row(&answer, "r2")["scores"],
        json!([
            { "name": "a", "mean": 0.5, "count": 2 },
            { "name": "b", "mean": 0.5, "count": 2 },
            { "name": "c", "mean": 0.5, "count": 2 },
        ])
    );
    assert_eq!(
        row(&answer, "r3")["scores"],
        json!([{ "name": "helpful", "mean": 0.5, "count": 1 }]),
        "the run of two releases, scored under each"
    );
}

/// A plan keeping `0` days of every organization's spans.
struct Keeps(u32);

#[async_trait]
impl Entitlements for Keeps {
    async fn grants(&self, _organization_id: &OrganizationId) -> Result<Grants, TelmoniError> {
        Ok(Grants::unlimited())
    }

    async fn capacity(
        &self,
        _organization_id: &OrganizationId,
    ) -> Result<Option<telmoni_shared::Capacity>, TelmoniError> {
        let allowance = |limit| telmoni_shared::Allowance {
            limit,
            entitlement: "larger_plan".into(),
            detail: "A larger plan holds more.".into(),
        };
        let month =
            Utc::now().duration_trunc(Duration::days(1)).expect("a day") - Duration::days(1);
        Ok(Some(telmoni_shared::Capacity {
            period_start: month,
            period_end: month + Duration::days(30),
            spans: allowance(1_000_000),
            spans_grace_days: 0,
            agent_questions: allowance(5),
            retention_days: self.0,
            content_bytes: 1 << 30,
            held_until: None,
            indexed_notices: 1_000,
        }))
    }
}

async fn settings(pool: &PgPool, l: &Lane, retention_days: Option<i16>) {
    sqlx::query(
        "INSERT INTO telemetry.project_settings (project_id, organization_id, retention_days)
         VALUES ($1, $2, $3)
         ON CONFLICT (project_id) DO UPDATE SET retention_days = EXCLUDED.retention_days",
    )
    .bind(&l.ours)
    .bind(&l.organization)
    .bind(retention_days)
    .execute(pool)
    .await
    .expect("a settings row");
}

/// A run of release `release` received `days` ago, scored three days ago
/// and now.
fn kept_for(l: &Lane, release: &'static str, days: i64) -> (Vec<SpanRow>, Vec<ScoreRow>) {
    let run = run(&l.ours, AGENT, days * 24 * 60);
    let then = ScoreRow {
        received_at: Utc::now().trunc_subsecs(6) - Duration::days(3),
        ..run.scored("helpful", 0.0)
    };
    (
        vec![run.own(("production", release), 1_000, false)],
        vec![then, run.scored("helpful", 1.0)],
    )
}

/// ⚠ The window is the Agents page's fourteen days, held to the project's
/// own retention, which the plan's does not know, and to the plan's: a run
/// or a score either store no longer promises is no part of a row.
#[sqlx::test]
async fn the_window_is_held_to_what_the_project_and_the_plan_keep(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let (new, scores_new) = kept_for(&l, "new", 1);
    let (old, scores_old) = kept_for(&l, "old", 3);
    l.keep(&[new, old].concat()).await;
    l.score(&[scores_new, scores_old].concat()).await;

    let whole = l.read(&[("agent", AGENT)]).await;
    assert_eq!(whole["days"], 14, "no settings row, and a plan keeping all");
    assert_eq!(values(&whole), ["new", "old"]);
    assert_eq!(
        row(&whole, "new")["scores"],
        json!([{ "name": "helpful", "mean": 0.5, "count": 2 }])
    );

    settings(&pool, &l, None).await;
    assert_eq!(l.read(&[("agent", AGENT)]).await["days"], 14);
    settings(&pool, &l, Some(2)).await;
    let held = l.read(&[("agent", AGENT)]).await;
    assert_eq!(held["days"], 2, "the project's own two days");
    assert_eq!(values(&held), ["new"], "a run three days old is past them");
    assert_eq!(
        row(&held, "new")["scores"],
        json!([{ "name": "helpful", "mean": 1.0, "count": 1 }]),
        "and a score given three days ago"
    );

    let Some(sold) = lane_under(&pool, module_path!(), Arc::new(Keeps(2))).await else {
        return;
    };
    let (new, scores_new) = kept_for(&sold, "new", 1);
    let (old, scores_old) = kept_for(&sold, "old", 3);
    sold.keep(&[new, old].concat()).await;
    sold.score(&[scores_new, scores_old].concat()).await;
    settings(&pool, &sold, Some(5)).await;
    let planned = sold.read(&[("agent", AGENT)]).await;
    assert_eq!(
        planned["days"], 2,
        "the plan's two days, under the project's five"
    );
    assert_eq!(values(&planned), ["new"]);
    settings(&pool, &sold, Some(1)).await;
    assert_eq!(
        sold.read(&[("agent", AGENT)]).await["days"],
        1,
        "the project's one, under the plan's two"
    );
}

/// The fifty newest versions are named, the answer saying it holds more,
/// and the runs naming none stay beside them.
#[sqlx::test]
async fn more_than_fifty_versions_are_cut_to_the_newest_and_say_so(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let releases: Vec<String> = (0..51).map(|n| format!("r{n:02}")).collect();
    let mut spans = Vec::new();
    for (n, release) in releases.iter().enumerate().skip(1) {
        let minutes = 100 - i64::try_from(n).expect("a count");
        spans.push(run(&l.ours, AGENT, minutes).own(("production", release), 1_000, false));
    }
    spans.push(run(&l.ours, AGENT, 1).own(PRODUCTION, 1_000, false));
    l.keep(&spans).await;

    let fifty = l.read(&[("agent", AGENT)]).await;
    assert_eq!(fifty["more"], false, "fifty are all named");
    assert_eq!(rows(&fifty).len(), 51, "and the runs naming none");

    l.keep(&[run(&l.ours, AGENT, 100).own(("production", "r00"), 1_000, false)])
        .await;
    let answer = l.read(&[("agent", AGENT)]).await;
    assert_eq!(answer["more"], true);
    let named = values(&answer);
    assert_eq!(named.len(), 51);
    assert_eq!(named.first(), Some(&"r50"), "the newest first");
    assert_eq!(named.get(49), Some(&"r01"), "the fiftieth");
    assert_eq!(named.last(), Some(&""), "the runs naming none, kept");
    assert!(!named.contains(&"r00"), "the oldest is the one left out");
}

/// ⚠ An agent's name is any text, and the lane reads each as it is named:
/// `..` and `a/b` would be another path, so the name is a parameter.
#[sqlx::test]
async fn an_agent_is_read_by_any_name_it_has(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let names = [
        ".",
        "..",
        "a/b",
        "%",
        "100%25",
        "a b",
        " padded ",
        "agént-日本",
        "a&b=c",
    ];
    let spans: Vec<SpanRow> = names
        .iter()
        .zip(1..)
        .map(|(name, minutes)| run(&l.ours, name, minutes).own(("production", *name), 1_000, false))
        .collect();
    l.keep(&spans).await;
    for name in names {
        let answer = l.read(&[("agent", name)]).await;
        assert_eq!(values(&answer), [name], "the agent named {name:?}");
        assert_eq!(row(&answer, name)["runs"], 1);
    }
    let absent = l.read(&[("agent", "never-ran")]).await;
    assert_eq!(absent["environment"], "");
    assert_eq!(absent["environments"], json!([]));
    assert_eq!(values(&absent), [] as [&str; 0]);

    let long = "a".repeat(129);
    for query in [
        String::new(),
        "?agent=".to_owned(),
        "?agent=a&agent=b".to_owned(),
        format!("?agent={long}"),
        "?agent=a&by=tool".to_owned(),
        "?agent=a&by=release&by=model".to_owned(),
        "?agent=a&environment=x&environment=y".to_owned(),
        "?agent=a&prompt=".to_owned(),
        "?agent=a&days=7".to_owned(),
    ] {
        assert_eq!(l.status(&query).await, StatusCode::BAD_REQUEST, "{query}");
    }
    assert_eq!(
        l.status(&format!("?agent={}", "a".repeat(128))).await,
        StatusCode::OK
    );
}

/// The lane is the project's readers': its read-only role reads it, and a
/// stranger, or another project's owner asking for ours, is refused.
#[sqlx::test]
async fn the_lane_is_the_projects_readers(pool: PgPool) {
    let Some(l) = lane(&pool, module_path!()).await else {
        return;
    };
    let path = path(&[("agent", AGENT)]);
    let (status, _) = l.ask(MEMBER, &l.ours, &path).await;
    assert_eq!(status, StatusCode::OK, "the read-only role");
    for (user, project) in [
        (STRANGER, &l.ours),
        (THEIR_OWNER, &l.ours),
        (MEMBER, &l.theirs),
    ] {
        let (status, _) = l.ask(user, project, &path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{user}");
    }
}
