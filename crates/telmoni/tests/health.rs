//! The composed process: the readiness probe reports every module's
//! database and the liveness probe none; the watchdog's heartbeat names the
//! loops no replica has beaten; every module's routes are mounted under one
//! listener, each behind its own gates.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test scaffolding")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Router, routing};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tower::ServiceExt;

use telmoni_auth::test_provider::test_issuer;
use telmoni_server::{App, Module, Parts};
use telmoni_shared::middleware::http::is_request_id;
use telmoni_shared::test_util::{captured_logs, unreachable_pool};
use telmoni_shared::types::API_TOKEN_PREFIX;

const SECRET: &str = "test-service-secret";

/// The whole process over pools that can never connect: every route mounts,
/// and nothing that touches a table can answer.
fn app() -> Arc<App> {
    Arc::new(assemble())
}

fn assemble() -> App {
    let auth_config = telmoni_auth::Config {
        database_url: String::new(),
        service_secret: SECRET.into(),
        service_secret_next: None,
        allow_test_session: false,
        redirect_uri: "http://localhost:3000/auth/callback".into(),
        app_url: "http://localhost:3000".into(),
        mail_from: "Telmoni <test@example.com>".into(),
        support_email: None,
        deletion_tail_budget_ms: 8_000,
    };
    let notifications_config = telmoni_notifications::Config {
        database_url: String::new(),
        service_secret: SECRET.into(),
        service_secret_next: None,
        app_url: "http://localhost:3000".into(),
        slack: None,
        slack_signing_secret: None,
        slack_api_base: "https://slack.com".into(),
        connector_kek: None,
        kms_api_base: "https://cloudkms.googleapis.com".into(),
        metadata_api_base: "http://metadata.google.internal".into(),
        delivery_poll_secs: 5,
        delivery_batch: 200,
        delivery_concurrency: 64,
        delivery_drain_rounds: 20,
        delivery_max_attempts: 5,
        delivery_backoff_secs: 30,
    };
    let auth_pool = unreachable_pool();
    let providers = telmoni_auth::Providers {
        issuer: test_issuer(auth_pool.clone()),
        password: None,
        external: None,
        mail: Arc::new(telmoni_shared::mail::NoopSender),
    };
    App::assemble(Parts {
        auth_config,
        auth_pool,
        providers,
        notifications_config,
        notifications_pool: unreachable_pool(),
        telemetry_config: telmoni_telemetry::Config {
            database_url: "".into(),
            clickhouse_url: "http://telemetry:telemetry@127.0.0.1:1".into(),
            telemetry_kek: format!("local:{}", "0".repeat(64)).into(),
            kms_api_base: "http://127.0.0.1:1".into(),
            metadata_api_base: "http://127.0.0.1:1".into(),
            redis_url: "redis://127.0.0.1:1".into(),
        },
        telemetry_pool: unreachable_pool(),
        agent: None,
        purge_hook: None,
        ownership_hook: None,
        entitlements: None,
        trusted_proxy_hops: 2,
    })
    .expect("the modules wire together")
}

/// A module of a deployment's own: one route, a scripted readiness, and a
/// count of how often the probe asked.
struct Probe {
    ready: bool,
    asked: AtomicUsize,
}

#[async_trait]
impl Module for Probe {
    fn name(&self) -> &'static str {
        "probe"
    }

    fn router(&self) -> Router {
        Router::new().route("/internal/v1/probe", routing::get(|| async { "probed" }))
    }

    async fn ready(&self) -> sqlx::Result<()> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        if self.ready {
            Ok(())
        } else {
            Err(sqlx::Error::PoolClosed)
        }
    }
}

async fn status(request: Request<Body>) -> StatusCode {
    app()
        .router()
        .oneshot(request)
        .await
        .expect("router responds")
        .status()
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn readiness_fails_when_a_database_is_unreachable() {
    assert_eq!(
        status(get("/health")).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn liveness_ignores_the_databases() {
    assert_eq!(status(get("/livez")).await, StatusCode::OK);
}

/// Every module's routes answer under one listener, each behind its own
/// gate: auth's `/me`, notifications' feed and telemetry's content mode all
/// refuse a request with no bearer, and the log-level lane refuses one with
/// no service secret — before any of them reaches a table it could not.
#[tokio::test]
async fn the_modules_are_mounted_behind_their_gates() {
    let me = Request::post("/internal/v1/me")
        .header("x-service-secret", SECRET)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(status(me).await, StatusCode::UNAUTHORIZED);

    let feed = Request::get("/internal/v1/notifications/feed")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", "org_1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status(feed).await, StatusCode::UNAUTHORIZED);

    let content_mode = Request::get("/internal/v1/telemetry/content-mode")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", "org_1")
        .header("x-project-id", "project_1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status(content_mode).await, StatusCode::UNAUTHORIZED);

    assert_eq!(
        status(get("/internal/v1/log-level")).await,
        StatusCode::UNAUTHORIZED
    );
    let level = Request::get("/internal/v1/log-level")
        .header("x-service-secret", SECRET)
        .body(Body::empty())
        .unwrap();
    assert_eq!(status(level).await, StatusCode::OK);

    let nowhere = send(get("/nowhere")).await;
    assert_eq!(
        nowhere.status(),
        StatusCode::NOT_FOUND,
        "an unmatched path is a 404, not a gate's 401"
    );
    let body = problem(nowhere).await;
    assert_eq!(body["type"], "/errors/not-found");
    assert_eq!(body["detail"], "no such lane");
}

/// A deployment with no agent still answers the console's agent lanes, so
/// the panel can say the operator has not configured it rather than fail:
/// the status says off, and a question is the agent's own 503. Both behind
/// the service secret, like every lane under `/internal/v1`.
#[tokio::test]
async fn without_an_agent_its_lanes_say_so() {
    assert_eq!(
        status(get("/internal/v1/agent/status")).await,
        StatusCode::UNAUTHORIZED
    );
    let response = app()
        .router()
        .oneshot(
            Request::get("/internal/v1/agent/status")
                .header("x-service-secret", SECRET)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], br#"{"enabled":false}"#);

    let turn = Request::post("/internal/v1/agent/turns")
        .header("x-service-secret", SECRET)
        .header("content-type", "application/json")
        .body(Body::from(r#"{"conversation_id":null,"message":"hi"}"#))
        .unwrap();
    assert_eq!(status(turn).await, StatusCode::SERVICE_UNAVAILABLE);
}

/// A module the binary mounts is served under the same listener and asked by
/// the same probe: its route answers, and its readiness is one of the reads
/// the probe makes, whatever the core's own answer.
#[tokio::test]
async fn a_mounted_module_is_served_and_probed() {
    let probe = Arc::new(Probe {
        ready: true,
        asked: AtomicUsize::new(0),
    });
    let mut app = assemble();
    app.mount(probe.clone());
    let app = Arc::new(app);

    let served = app
        .router()
        .oneshot(get("/internal/v1/probe"))
        .await
        .expect("router responds");
    assert_eq!(served.status(), StatusCode::OK);

    let health = app
        .router()
        .oneshot(get("/health"))
        .await
        .expect("router responds");
    assert_eq!(
        health.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the core's unreachable pools still fail the probe"
    );
    assert_eq!(
        probe.asked.load(Ordering::SeqCst),
        1,
        "the module was asked once"
    );
}

/// A module whose loops a test scripts stale or beaten, counting how often
/// the watchdog asked.
struct Looping {
    stale: Vec<String>,
    asked: AtomicUsize,
}

#[async_trait]
impl Module for Looping {
    fn name(&self) -> &'static str {
        "looping"
    }

    fn router(&self) -> Router {
        Router::new()
    }

    async fn ready(&self) -> sqlx::Result<()> {
        Ok(())
    }

    async fn stale(&self) -> Vec<String> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.stale.clone()
    }
}

/// A process running `stale`'s loops beside the core's, which run none
/// until `serve` starts them.
fn looping(stale: &[&str]) -> (Arc<App>, Arc<Looping>) {
    let module = Arc::new(Looping {
        stale: stale.iter().map(|name| (*name).to_owned()).collect(),
        asked: AtomicUsize::new(0),
    });
    let mut app = assemble();
    app.mount(module.clone());
    (Arc::new(app), module)
}

async fn body(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("a body");
    serde_json::from_slice(&bytes).expect("JSON")
}

/// ⚠ The watchdog's answer names each stale loop and nothing else, and none
/// is a `200`; it reads no database for a loop the process does not run, so
/// unreachable pools are no page.
#[tokio::test]
async fn the_heartbeat_names_the_loops_no_replica_has_beaten() {
    let (app, _) = looping(&[]);
    let answer = app.router().oneshot(get("/heartbeat")).await.unwrap();
    assert_eq!(answer.status(), StatusCode::OK);
    assert_eq!(body(answer).await, serde_json::json!({ "status": "ok" }));

    let (app, _) = looping(&["billing.reconcile"]);
    let answer = app.router().oneshot(get("/heartbeat")).await.unwrap();
    assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        body(answer).await,
        serde_json::json!({ "status": "stale", "stale": ["billing.reconcile"] })
    );
}

/// A flood of the watchdog's asks reads the loops once a few seconds.
#[tokio::test]
async fn the_heartbeat_reads_the_loops_once_for_a_flood() {
    let (app, module) = looping(&["billing.reconcile"]);
    for _ in 0..20 {
        let answer = app.router().oneshot(get("/heartbeat")).await.unwrap();
        assert_eq!(answer.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
    assert_eq!(module.asked.load(Ordering::SeqCst), 1);
}

async fn send(request: Request<Body>) -> axum::response::Response {
    app()
        .router()
        .oneshot(request)
        .await
        .expect("router responds")
}

fn header<'a>(response: &'a axum::response::Response, name: &str) -> Option<&'a str> {
    response.headers().get(name).and_then(|v| v.to_str().ok())
}

async fn problem(response: axum::response::Response) -> serde_json::Value {
    assert_eq!(
        header(&response, "content-type"),
        Some("application/problem+json")
    );
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).expect("a problem document")
}

/// OTLP's `Status`, as ingest answers a refusal.
async fn otlp_status(response: axum::response::Response) -> serde_json::Value {
    assert_eq!(header(&response, "content-type"), Some("application/json"));
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&body).expect("an OTLP status")
}

/// The public read API is on the binary's `/v1`, open to a machine with no
/// service secret and refused without a key: a problem document with the
/// bearer challenge; a method no lane takes and a path none serves are
/// problem documents too, and `OPTIONS` is told the methods.
#[tokio::test]
async fn the_public_api_is_served_at_v1_behind_its_key() {
    let refused = send(get("/v1/organization")).await;
    assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(header(&refused, "www-authenticate"), Some("Bearer"));
    let body = problem(refused).await;
    assert_eq!(body["type"], "/errors/auth/invalid-token");
    assert_eq!(body["detail"], "a live tm_ API key is required");

    let put = Request::put("/v1/organization")
        .body(Body::empty())
        .unwrap();
    let refused = send(put).await;
    assert_eq!(refused.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&refused, "allow"), Some("GET, HEAD, OPTIONS"));
    assert_eq!(problem(refused).await["type"], "/errors/method-not-allowed");

    let options = Request::options("/v1/members").body(Body::empty()).unwrap();
    let told = send(options).await;
    assert_eq!(told.status(), StatusCode::NO_CONTENT);
    assert_eq!(header(&told, "allow"), Some("GET, HEAD, OPTIONS"));

    let missing = send(get("/v1/audit")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem(missing).await["type"], "/errors/not-found");

    let root = send(get("/v1/")).await;
    assert_eq!(root.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem(root).await["type"], "/errors/not-found");
}

/// The CLI's lanes are on the binary's `/cli/v1`, each taking `POST` and its
/// own credential: a bearer lane with no bearer is a 401 before any table,
/// a browser is refused, a body past the cap is refused, and a path none
/// serves is a problem document of the door's own.
#[tokio::test]
async fn the_cli_lanes_are_served_at_cli_behind_their_gates() {
    let post = |path: &str| Request::post(path).header("content-type", "application/json");

    let no_bearer = send(post("/cli/v1/me").body(Body::empty()).unwrap()).await;
    assert_eq!(no_bearer.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        problem(no_bearer).await["type"],
        "/errors/auth/unauthenticated"
    );

    let browser = send(
        post("/cli/v1/auth/refresh")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(r#"{"refreshToken":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(browser.status(), StatusCode::FORBIDDEN);
    assert_eq!(problem(browser).await["detail"], "not a browser lane");

    let get_me = send(get("/cli/v1/me")).await;
    assert_eq!(get_me.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&get_me, "allow"), Some("POST"));
    assert_eq!(problem(get_me).await["type"], "/errors/method-not-allowed");

    let nowhere = send(post("/cli/v1/nowhere").body(Body::empty()).unwrap()).await;
    assert_eq!(nowhere.status(), StatusCode::NOT_FOUND);
    let body = problem(nowhere).await;
    assert_eq!(body["type"], "/errors/not-found");
    assert_eq!(body["detail"], "no such lane");

    let root = send(post("/cli/v1/").body(Body::empty()).unwrap()).await;
    assert_eq!(root.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem(root).await["type"], "/errors/not-found");

    let unversioned = send(post("/cli/me").body(Body::empty()).unwrap()).await;
    assert_eq!(unversioned.status(), StatusCode::NOT_FOUND);

    let oversized = format!(r#"{{"refreshToken":"{}"}}"#, "x".repeat(9 * 1024));
    let too_large = send(
        post("/cli/v1/auth/refresh")
            .body(Body::from(oversized))
            .unwrap(),
    )
    .await;
    assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

/// The door meters each lane by the caller's address, read past the two
/// proxies the tier runs, and refuses the one over the minute's allowance
/// with how long to wait; another address has an allowance of its own.
#[tokio::test]
async fn the_door_meters_the_cli_lanes_by_address() {
    // One process: the door's limits live with it, held in the process
    // while its Redis (unreachable here) does not answer.
    let app = app();
    let router = app.router();
    let from = |address: &str| {
        router.clone().oneshot(
            Request::post("/cli/v1/nowhere")
                .header("x-forwarded-for", format!("{address}, 35.191.0.1"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    for n in 0..120 {
        assert_eq!(
            from("203.0.113.9").await.expect("router responds").status(),
            StatusCode::NOT_FOUND,
            "request {n} is within the minute's allowance"
        );
    }
    // The limit leaks two a second, so a slow machine earns a few more
    // before the refusal; it comes within a handful either way.
    let mut refused = None;
    for _ in 0..20 {
        let response = from("203.0.113.9").await.expect("router responds");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(response);
            break;
        }
    }
    let refused = refused.expect("the minute's allowance was spent");
    assert!(header(&refused, "retry-after").is_some());
    let body = problem(refused).await;
    assert_eq!(body["type"], "/errors/tenant/rate-limited");

    assert_eq!(
        from("198.51.100.7")
            .await
            .expect("router responds")
            .status(),
        StatusCode::NOT_FOUND,
        "another address is metered on its own"
    );
}

/// Ingest is on the binary's `/otlp`, behind the door, which meters it by
/// the caller's address before the lane looks its key up: an export with no
/// key is the lane's 401, in OTLP's shape, until the address's minute is
/// spent, and the door's refusal is in the lane's shape too, with
/// `Retry-After`; another address has a minute of its own. The door reads
/// no credential here, since the key's limit is the lane's, per project.
#[tokio::test]
async fn the_door_meters_ingest_by_address_before_its_key() {
    let app = app();
    let router = app.router();
    let from = |address: &str| {
        router.clone().oneshot(
            Request::post("/otlp/v1/traces")
                .header("content-type", "application/json")
                .header("x-forwarded-for", format!("{address}, 35.191.0.1"))
                .body(Body::from("{}"))
                .unwrap(),
        )
    };
    let unkeyed = from("203.0.113.9").await.expect("router responds");
    assert_eq!(unkeyed.status(), StatusCode::UNAUTHORIZED);
    let body = otlp_status(unkeyed).await;
    assert_eq!(body["code"], 16);
    assert_eq!(body["message"], "/errors/auth/invalid-token");
    for n in 1..1_200 {
        assert_eq!(
            from("203.0.113.9").await.expect("router responds").status(),
            StatusCode::UNAUTHORIZED,
            "export {n} is within the minute's allowance"
        );
    }
    // The limit leaks twenty a second, so a slow machine earns a few more
    // before the refusal; it comes within a handful either way.
    let mut refused = None;
    for _ in 0..200 {
        let response = from("203.0.113.9").await.expect("router responds");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(response);
            break;
        }
    }
    let refused = refused.expect("the minute's allowance was spent");
    assert!(header(&refused, "retry-after").is_some());
    let body = otlp_status(refused).await;
    assert_eq!(body["code"], 8);
    assert!(
        body["message"]
            .as_str()
            .is_some_and(|message| message.starts_with("/errors/tenant/rate-limited")),
        "{body}"
    );

    assert_eq!(
        from("198.51.100.7")
            .await
            .expect("router responds")
            .status(),
        StatusCode::UNAUTHORIZED,
        "another address is metered on its own"
    );
}

/// What ingest does not serve, `/otlp/` too, is the lane's to answer in OTLP's
/// shape, and a stray path spends none of the address's allowance, so an
/// SDK's logs and metrics, sent beside its traces, never cost them their room.
#[tokio::test]
async fn ingest_answers_what_it_does_not_serve_outside_the_doors_allowance() {
    let app = app();
    let router = app.router();
    let send = |method: &str, path: &str| {
        router.clone().oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("content-type", "application/json")
                .header("x-forwarded-for", "203.0.113.11, 35.191.0.1")
                .body(Body::from("{}"))
                .unwrap(),
        )
    };
    for path in ["/otlp", "/otlp/", "/otlp/v1/logs", "/otlp/v1/metrics"] {
        let missing = send("POST", path).await.expect("router responds");
        assert_eq!(missing.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(otlp_status(missing).await["code"], 5, "{path}");
    }
    let wrong = send("PUT", "/otlp/v1/traces")
        .await
        .expect("router responds");
    assert_eq!(wrong.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&wrong, "allow"), Some("POST"));
    assert_eq!(otlp_status(wrong).await["code"], 12);

    for n in 0..1_300 {
        assert_eq!(
            send("POST", "/otlp/v1/logs")
                .await
                .expect("router responds")
                .status(),
            StatusCode::NOT_FOUND,
            "stray export {n} is never metered"
        );
    }
    assert_eq!(
        send("POST", "/otlp/v1/traces")
            .await
            .expect("router responds")
            .status(),
        StatusCode::UNAUTHORIZED,
        "the traces' allowance is whole"
    );
}

/// The door's refusal of an export waits for its body, as the lane's own do:
/// hyper's close over one still coming breaks the upload, which an exporter
/// takes for no answer.
#[tokio::test]
async fn the_doors_refusal_of_an_export_waits_for_its_body() {
    const FROM: &str = "203.0.113.10, 35.191.0.1";
    let router = app().router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a port");
    let address = listener.local_addr().expect("its address");
    tokio::spawn({
        let router = router.clone();
        async move { axum::serve(listener, router).await }
    });
    // The spent minute leaks one a 50 ms, which can let the probe in, to the
    // lane's own 401 after its body as well: spent again and probed again.
    for _ in 0..5 {
        let mut spent = false;
        for _ in 0..1_400 {
            let response = router
                .clone()
                .oneshot(
                    Request::post("/otlp/v1/traces")
                        .header("content-type", "application/json")
                        .header("x-forwarded-for", FROM)
                        .body(Body::from("{}"))
                        .unwrap(),
                )
                .await
                .expect("router responds");
            if response.status() == StatusCode::TOO_MANY_REQUESTS {
                spent = true;
                break;
            }
        }
        assert!(spent, "the minute's allowance was spent");

        let mut stream = tokio::net::TcpStream::connect(address)
            .await
            .expect("a connection");
        let half = vec![b' '; 300_000];
        let head = format!(
            "POST /otlp/v1/traces HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\
             content-type: application/json\r\nx-forwarded-for: {FROM}\r\ncontent-length: {}\r\n\r\n",
            2 * half.len()
        );
        stream.write_all(head.as_bytes()).await.expect("the head");
        stream.write_all(&half).await.expect("the first half");
        assert!(
            tokio::time::timeout(Duration::from_millis(200), stream.peek(&mut [0u8; 1]))
                .await
                .is_err(),
            "answered with half the body still to come"
        );
        stream.write_all(&half).await.expect("the second half");
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer))
            .await
            .expect("answered within ten seconds")
            .expect("the answer");
        let answer = String::from_utf8_lossy(&answer);
        if answer.starts_with("HTTP/1.1 429 ") {
            return;
        }
        assert!(answer.starts_with("HTTP/1.1 401 "), "{answer}");
    }
    panic!("the door let every probe in");
}

/// ⚠ A request is named by this process alone: an id sent to a public lane,
/// free text or well formed, gives way to one minted here, the id the
/// response echoes; and one sent with the service secret beside it does too,
/// so no answer on a public lane tells a guessed secret from a wrong one.
#[tokio::test]
async fn a_request_is_named_by_this_process_alone() {
    for sent in ["it's free text, of any length", "req-from-the-caller"] {
        let asked = send(
            Request::get("/v1/organization")
                .header("x-request-id", sent)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let named = header(&asked, "x-request-id").expect("an id echoed");
        assert_ne!(named, sent, "the caller named its request");
        assert!(is_request_id(named), "{named}");
    }

    for secret in [SECRET, "a-guess"] {
        let probed = send(
            Request::get("/v1/organization")
                .header("x-service-secret", secret)
                .header("x-request-id", "req-probe")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_ne!(
            header(&probed, "x-request-id"),
            Some("req-probe"),
            "the answer told whether {secret} is the secret"
        );
    }
}

/// ⚠ No log line carries an API key pasted into a path: the span the binary
/// opens for every request, before the door and the lanes, prints the path
/// with the key cut out — on a lane's 404 and on the server's for a lane
/// misspelt, the likeliest leak, as a shell's join leaves one.
#[tokio::test]
async fn no_log_line_carries_an_api_key_in_a_path() {
    let (_logging, logs) = captured_logs();
    let key = format!("{API_TOKEN_PREFIX}k3yk3yk3yk3yk3yk3yk3yk3y");
    for (path, printed) in [
        (format!("/v1/{key}"), "/v1/…"),
        (format!("/v1/members/{key}"), "/v1/members/…"),
        (format!("/v2/{key}/members"), "/v2/…/members"),
    ] {
        let before = logs.text().len();
        assert_eq!(
            send(get(&path)).await.status(),
            StatusCode::NOT_FOUND,
            "{path}"
        );
        let own = logs.text().split_off(before);
        assert!(
            own.contains(&format!(r#""path":"{printed}""#)),
            "{path} was not logged as {printed}: {own}"
        );
    }
    assert!(
        !logs.text().contains("k3yk3yk3y"),
        "an API key reached a log line"
    );
}
