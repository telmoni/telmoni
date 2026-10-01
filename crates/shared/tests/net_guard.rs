//! The outbound guard, end to end through a real client: the resolver half and
//! the IP-literal half. The far end is a loopback mock that must never be reached.

use std::sync::Arc;
use std::time::Duration;

use telmoni_shared::net_guard::{Egress, GuardedResolver, HostRejection};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn far_end() -> MockServer {
    let mocks = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mocks)
        .await;
    mocks
}

/// `localhost` is a NAME, so the resolver drops its loopback answers: the DNS-rebind half.
#[tokio::test]
async fn the_resolver_refuses_a_name_that_resolves_only_to_loopback() {
    let mocks = far_end().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .dns_resolver(Arc::new(GuardedResolver))
        .build()
        .unwrap();
    let url = format!("http://localhost:{}/hook", mocks.address().port());

    let err = client.post(&url).send().await.expect_err("never dialled");
    let text = format!("{err:?}");
    assert!(
        text.contains("resolves only to non-global addresses"),
        "{text}"
    );
    assert!(mocks.received_requests().await.unwrap().is_empty());
}

/// An IP literal never reaches a resolver, so the client checks the URL before connecting.
#[tokio::test]
async fn the_guarded_client_refuses_a_literal_and_a_local_name_before_the_socket() {
    let mocks = far_end().await;
    let egress = Egress::guarded(Duration::from_secs(5), "telmoni-test").unwrap();
    assert!(egress.is_guarded());

    let literal = format!("{}/hook", mocks.uri());
    assert_eq!(
        egress.post(&literal).err(),
        Some(HostRejection::NotGlobal),
        "{literal}"
    );
    let named = format!("http://localhost:{}/hook", mocks.address().port());
    assert_eq!(egress.post(&named).err(), Some(HostRejection::Local));
    assert_eq!(
        egress.delete("https://169.254.169.254/latest").err(),
        Some(HostRejection::NotGlobal)
    );
    assert_eq!(
        egress.post("not a url").err(),
        Some(HostRejection::Malformed)
    );
    assert!(mocks.received_requests().await.unwrap().is_empty());

    assert!(egress.post("https://hooks.example.com/x").is_ok());
}

/// The seam the service suites use: without the guard, loopback is an ordinary far end.
#[tokio::test]
async fn an_unguarded_client_reaches_loopback() {
    let mocks = far_end().await;
    let egress = Egress::unguarded(reqwest::Client::new());
    assert!(!egress.is_guarded());
    let resp = egress
        .post(&format!("{}/hook", mocks.uri()))
        .unwrap()
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(mocks.received_requests().await.unwrap().len(), 1);
}
