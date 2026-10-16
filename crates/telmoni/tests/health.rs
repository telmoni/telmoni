//! The composed process: the readiness probe reports every module's
//! database and the liveness probe none; every module's routes are mounted
//! under one listener, each behind its own gates.
#![expect(clippy::unwrap_used, clippy::expect_used, reason = "test scaffolding")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::{Router, routing};
use tower::ServiceExt;

use telmoni::{App, Module, Parts};
use telmoni_auth::test_provider::test_issuer;
use telmoni_shared::middleware::http::is_request_id;
use telmoni_shared::test_util::{captured_logs, unreachable_pool};
use telmoni_shared::types::{PING_KEY_PREFIX, mint_ping_key};

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
        Router::new().route("/internal/probe", routing::get(|| async { "probed" }))
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
    let me = Request::post("/me")
        .header("x-service-secret", SECRET)
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    assert_eq!(status(me).await, StatusCode::UNAUTHORIZED);

    let feed = Request::get("/internal/notifications/feed")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", "org_1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status(feed).await, StatusCode::UNAUTHORIZED);

    let content_mode = Request::get("/internal/telemetry/content-mode")
        .header("x-service-secret", SECRET)
        .header("x-organization-id", "org_1")
        .header("x-project-id", "project_1")
        .body(Body::empty())
        .unwrap();
    assert_eq!(status(content_mode).await, StatusCode::UNAUTHORIZED);

    assert_eq!(
        status(get("/internal/log-level")).await,
        StatusCode::UNAUTHORIZED
    );
    let level = Request::get("/internal/log-level")
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
/// the service secret, like every lane under `/internal`.
#[tokio::test]
async fn without_an_agent_its_lanes_say_so() {
    assert_eq!(
        status(get("/internal/agent/status")).await,
        StatusCode::UNAUTHORIZED
    );
    let response = app()
        .router()
        .oneshot(
            Request::get("/internal/agent/status")
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

    let turn = Request::post("/internal/agent/turns")
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
        .oneshot(get("/internal/probe"))
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
    assert_eq!(body["detail"], "a live telmoni_ API key is required");

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

/// The CLI's lanes are on the binary's `/cli`, each taking `POST` and its
/// own credential: a bearer lane with no bearer is a 401 before any table,
/// a browser is refused, a body past the cap is refused, and a path none
/// serves is a problem document of the door's own.
#[tokio::test]
async fn the_cli_lanes_are_served_at_cli_behind_their_gates() {
    let post = |path: &str| Request::post(path).header("content-type", "application/json");

    let no_bearer = send(post("/cli/me").body(Body::empty()).unwrap()).await;
    assert_eq!(no_bearer.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        problem(no_bearer).await["type"],
        "/errors/auth/unauthenticated"
    );

    let browser = send(
        post("/cli/auth/refresh")
            .header("sec-fetch-site", "same-origin")
            .body(Body::from(r#"{"refreshToken":"x"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(browser.status(), StatusCode::FORBIDDEN);
    assert_eq!(problem(browser).await["detail"], "not a browser lane");

    let get_me = send(get("/cli/me")).await;
    assert_eq!(get_me.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(header(&get_me, "allow"), Some("POST"));
    assert_eq!(problem(get_me).await["type"], "/errors/method-not-allowed");

    let nowhere = send(post("/cli/nowhere").body(Body::empty()).unwrap()).await;
    assert_eq!(nowhere.status(), StatusCode::NOT_FOUND);
    let body = problem(nowhere).await;
    assert_eq!(body["type"], "/errors/not-found");
    assert_eq!(body["detail"], "no such lane");

    let root = send(post("/cli/").body(Body::empty()).unwrap()).await;
    assert_eq!(root.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem(root).await["type"], "/errors/not-found");

    let oversized = format!(r#"{{"refreshToken":"{}"}}"#, "x".repeat(9 * 1024));
    let too_large = send(
        post("/cli/auth/refresh")
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
            Request::post("/cli/nowhere")
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

/// The key with its last character percent-encoded: one key to the lane,
/// which reads its path decoded, and no key to the door.
fn percent_encoded_last(key: &str) -> String {
    let last = key.bytes().next_back().expect("a key has a last byte");
    let head = key
        .strip_suffix(char::from(last))
        .expect("the key ends in its last byte");
    format!("{head}%{last:02X}")
}

/// The ping lane is on the binary's `/ping`, behind the door, which reads
/// the key from the path as it came: a key spelled as minted passes to the
/// lane, which asks the database for it (unreachable here); any other
/// spelling in the key's place — the key with a character percent-encoded,
/// a word — is refused at the door with the lane's own answer for a key it
/// does not hold, so an encoding of a key is never let in on a bucket of
/// its own; nothing in the key's place, or a doubled slash before it, is
/// the lane's own 404, and the lane's root with its trailing slash the
/// server's. The lane's answers and the door's refusals alike reach the
/// caller saying no cache may keep them.
#[tokio::test]
async fn the_ping_lane_is_served_at_ping_behind_the_door() {
    let key = mint_ping_key();
    let asked = send(get(&format!("/ping/{key}/nightly"))).await;
    assert_eq!(
        asked.status(),
        StatusCode::INTERNAL_SERVER_ERROR,
        "a key as minted reaches the lane, which asks the database"
    );
    assert_eq!(
        header(&asked, "cache-control"),
        Some("no-store"),
        "the lane's answer, through the door"
    );
    assert_eq!(problem(asked).await["type"], "/errors/internal/database");

    // The door's refusal is the lane's answer for a key it does not hold,
    // none a cache may keep either: one that took `%41` for `A` would keep
    // it as the answer for the key itself.
    let encoded = percent_encoded_last(&key);
    let refused = send(get(&format!("/ping/{encoded}/nightly"))).await;
    assert_eq!(refused.status(), StatusCode::NOT_FOUND);
    assert_eq!(header(&refused, "cache-control"), Some("no-store"));
    let body = problem(refused).await;
    assert_eq!(body["type"], "/errors/auth/not-found");
    assert_eq!(body["detail"], "no ping key by that value");

    let word = send(get("/ping/notakey/nightly")).await;
    assert_eq!(word.status(), StatusCode::NOT_FOUND);
    assert_eq!(header(&word, "cache-control"), Some("no-store"));
    assert_eq!(problem(word).await["detail"], "no ping key by that value");

    // The lane's own 404 for a path under it with no key where one goes,
    // and the server's for the lane's root with its trailing slash, which
    // the nesting's catch-all does not take: nothing to count either way.
    let doubled = format!("/ping//{key}/nightly");
    for nothing in ["/ping", doubled.as_str()] {
        let response = send(get(nothing)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{nothing}");
        assert_eq!(
            header(&response, "cache-control"),
            Some("no-store"),
            "{nothing}: the lane's fallback, through the door"
        );
        assert_eq!(
            problem(response).await["detail"],
            "no such lane under /ping",
            "{nothing}"
        );
    }
    let root = send(get("/ping/")).await;
    assert_eq!(root.status(), StatusCode::NOT_FOUND);
    assert_eq!(problem(root).await["detail"], "no such lane");
}

/// The door meters the ping lane by the key, whatever addresses spend it,
/// and the key is one bucket however it is sent: with its minute spent, the
/// same key with a character percent-encoded is refused as no key rather
/// than let in on a bucket of its own, while another key has a minute of
/// its own. A method the lane does not take is answered behind the door
/// with no database, so the count is the door's alone.
#[tokio::test]
async fn the_door_meters_the_ping_lane_by_key_from_any_address() {
    let app = app();
    let router = app.router();
    let key = mint_ping_key();
    let put = |path: String, address: &str| {
        router.clone().oneshot(
            Request::put(path)
                .header("x-forwarded-for", format!("{address}, 35.191.0.1"))
                .body(Body::empty())
                .unwrap(),
        )
    };
    let mut addresses = [
        "203.0.113.9",
        "203.0.113.10",
        "198.51.100.7",
        "198.51.100.8",
    ]
    .into_iter()
    .cycle();
    for n in 0..600 {
        let address = addresses.next().expect("the addresses cycle");
        assert_eq!(
            put(format!("/ping/{key}/nightly"), address)
                .await
                .expect("router responds")
                .status(),
            StatusCode::METHOD_NOT_ALLOWED,
            "request {n} is within the key's minute"
        );
    }
    // The limit leaks ten a second, so a slow machine earns one more for
    // every tenth of a second the burst took; the loop ends at the first
    // refusal either way, and a limit that never refuses runs it out.
    let mut refused = None;
    for _ in 0..600 {
        let address = addresses.next().expect("the addresses cycle");
        let response = put(format!("/ping/{key}/nightly"), address)
            .await
            .expect("router responds");
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            refused = Some(response);
            break;
        }
    }
    let refused = refused.expect("the key's minute was spent");
    assert!(header(&refused, "retry-after").is_some());
    assert_eq!(header(&refused, "cache-control"), Some("no-store"));
    assert_eq!(
        problem(refused).await["type"],
        "/errors/tenant/rate-limited"
    );

    let encoded = percent_encoded_last(&key);
    let spelled_otherwise = put(format!("/ping/{encoded}/nightly"), "203.0.113.11")
        .await
        .expect("router responds");
    assert_eq!(
        spelled_otherwise.status(),
        StatusCode::NOT_FOUND,
        "an encoding of the key is no bucket of its own"
    );

    let other = mint_ping_key();
    assert_eq!(
        put(format!("/ping/{other}/nightly"), "203.0.113.9")
            .await
            .expect("router responds")
            .status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "another key is metered on its own"
    );
}

/// ⚠ A request is named by this process alone: an id sent to the ping lane,
/// free text or well formed, gives way to one minted here, the id the
/// response echoes; and one sent with the service secret beside it does too,
/// so no answer on a public lane tells a guessed secret from a wrong one.
#[tokio::test]
async fn a_request_is_named_by_this_process_alone() {
    let key = mint_ping_key();
    for sent in ["it's free text, of any length", "req-from-the-caller"] {
        let pinged = send(
            Request::get(format!("/ping/{key}/nightly"))
                .header("x-request-id", sent)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        let named = header(&pinged, "x-request-id").expect("an id echoed");
        assert_ne!(named, sent, "the caller named its request");
        assert!(is_request_id(named), "{named}");
    }

    for secret in [SECRET, "a-guess"] {
        let probed = send(
            Request::get(format!("/ping/{key}/nightly"))
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

/// ⚠ No log line carries a ping's key, at any level, whoever writes it: the
/// span the binary opens for every request, before the door and the lanes,
/// prints the path with the key cut out — on the lane's own failure, which
/// it logs at error inside the span, on the lane's fallback, on the door's
/// refusal of the key spelled otherwise, and on the server's 404 for a lane
/// misspelt, the likeliest leak, as a shell's join leaves one.
#[tokio::test]
async fn no_log_line_carries_a_pings_key() {
    let (_logging, logs) = captured_logs();
    let key = mint_ping_key();
    let encoded = percent_encoded_last(&key);
    for (path, answered, printed) in [
        (
            format!("/ping/{key}/nightly"),
            StatusCode::INTERNAL_SERVER_ERROR,
            "/ping/…/nightly",
        ),
        (
            format!("/ping//{key}/nightly"),
            StatusCode::NOT_FOUND,
            "/ping//…/nightly",
        ),
        (
            format!("/ping/{encoded}/nightly"),
            StatusCode::NOT_FOUND,
            "/ping/…/nightly",
        ),
        (
            format!("/PING/{key}/nightly"),
            StatusCode::NOT_FOUND,
            "/PING/…/nightly",
        ),
        (
            format!("/pings/{key}/nightly"),
            StatusCode::NOT_FOUND,
            "/pings/…/nightly",
        ),
    ] {
        let before = logs.text().len();
        assert_eq!(send(get(&path)).await.status(), answered, "{path}");
        let own = logs.text().split_off(before);
        assert!(
            own.contains(&format!(r#""path":"{printed}""#)),
            "{path} was not logged as {printed}: {own}"
        );
    }
    let lines = logs.text();
    let refused = lines
        .lines()
        .find(|line| line.contains("ping: refused on our side"))
        .expect("the lane's own line at error");
    assert!(
        refused.contains(r#""path":"/ping/…/nightly""#),
        "the lane's line was written outside the request's span: {refused}"
    );
    // The key less its last character, so its encoded spelling counts too.
    let secret: String = key
        .chars()
        .skip(PING_KEY_PREFIX.len())
        .take(key.len() - PING_KEY_PREFIX.len() - 1)
        .collect();
    assert!(!lines.contains(&secret), "a ping's key reached a log line");
}
