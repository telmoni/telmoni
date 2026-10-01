//! The Cloud KMS adapter against a mock: the bearer, the two bodies, and the
//! classification — only `INVALID_ARGUMENT` means a broken row; everything else holds.
#![expect(
    clippy::unwrap_used,
    reason = "test scaffolding: asserts and fixture setup"
)]

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use wiremock::matchers::{body_partial_json, header as header_is, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use telmoni_shared::envelope::{KEK_VERSION, Kek, KekError, KmsKek, Vault};

const KEY: &str =
    "projects/telmoni-test/locations/us-central1/keyRings/telmoni/cryptoKeys/connector-kek";
const BEARER: &str = "ya29.test-bearer-that-must-never-print";
const TOKEN_PATH: &str = "/computeMetadata/v1/instance/service-accounts/default/token";
/// What KMS hands back for a wrap: opaque bytes, stored exactly as given.
const WRAPPED: [u8; 40] = [0xA5; 40];
/// What KMS hands back for an unwrap: a 32-byte data key.
const DEK: [u8; 32] = [0x42; 32];
const ROW: [u8; 16] = [0x11; 16];

fn kek(mocks: &MockServer) -> Kek {
    Kek::Kms(KmsKek::new(KEY, &mocks.uri(), &mocks.uri()).unwrap())
}

fn http() -> reqwest::Client {
    reqwest::Client::new()
}

fn encrypt_path() -> String {
    format!("/v1/{KEY}:encrypt")
}

fn decrypt_path() -> String {
    format!("/v1/{KEY}:decrypt")
}

async fn mount_token(mocks: &MockServer, times: u64) {
    Mock::given(method("GET"))
        .and(path(TOKEN_PATH))
        .and(header_is("metadata-flavor", "Google"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": BEARER,
            "expires_in": 3599,
            "token_type": "Bearer"
        })))
        .expect(times)
        .mount(mocks)
        .await;
}

fn kms_error(status: u16, code: &str, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(serde_json::json!({
        "error": { "code": status, "message": message, "status": code }
    }))
}

/// The bearer is fetched once and reused; the encrypt body binds the row id as AAD.
#[tokio::test]
async fn a_wrap_posts_the_row_id_as_aad_under_one_bearer_reused_across_calls() {
    let mocks = MockServer::start().await;
    mount_token(&mocks, 1).await;
    let dek = Vault::new().new_dek().unwrap();
    Mock::given(method("POST"))
        .and(path(encrypt_path()))
        .and(header_is("authorization", &format!("Bearer {BEARER}")))
        .and(header_is("content-type", "application/json"))
        .and(body_partial_json(serde_json::json!({
            "plaintext": STANDARD.encode(dek.as_bytes()),
            "additionalAuthenticatedData": STANDARD.encode(ROW),
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "name": format!("{KEY}/cryptoKeyVersions/1"),
            "ciphertext": STANDARD.encode(WRAPPED),
            "ciphertextCrc32c": "0",
            "protectionLevel": "SOFTWARE"
        })))
        .expect(2)
        .mount(&mocks)
        .await;
    let kek = kek(&mocks);

    let first = kek.wrap(&http(), &dek, &ROW).await.unwrap();
    let second = kek.wrap(&http(), &dek, &ROW).await.unwrap();
    assert_eq!(first, WRAPPED);
    assert_eq!(second, WRAPPED);
}

/// The decrypt body carries the stored bytes and the row id; the answer is the data key.
#[tokio::test]
async fn an_unwrap_round_trips_through_the_documented_bodies() {
    let mocks = MockServer::start().await;
    mount_token(&mocks, 1).await;
    Mock::given(method("POST"))
        .and(path(decrypt_path()))
        .and(header_is("authorization", &format!("Bearer {BEARER}")))
        .and(body_partial_json(serde_json::json!({
            "ciphertext": STANDARD.encode(WRAPPED),
            "additionalAuthenticatedData": STANDARD.encode(ROW),
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "plaintext": STANDARD.encode(DEK),
            "plaintextCrc32c": "0",
            "usedPrimary": true,
            "protectionLevel": "SOFTWARE"
        })))
        .expect(1)
        .mount(&mocks)
        .await;

    let dek = kek(&mocks)
        .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION)
        .await
        .unwrap();
    assert_eq!(dek.as_bytes(), &DEK);
}

/// A foreign key version is refused before any call: no token, no decrypt.
#[tokio::test]
async fn a_foreign_key_version_is_refused_without_a_call() {
    let mocks = MockServer::start().await;
    let err = kek(&mocks)
        .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION + 1)
        .await
        .unwrap_err();
    assert!(matches!(err, KekError::Invalid(_)), "{err}");
    assert!(mocks.received_requests().await.unwrap().is_empty());
}

/// The load-bearing table. Only `INVALID_ARGUMENT` names the row; every other
/// answer holds, and no answer of any kind prints the bearer.
#[tokio::test]
async fn every_answer_but_invalid_argument_holds_and_none_prints_the_bearer() {
    let cases: [(u16, &str, &str, bool); 6] = [
        (
            503,
            "UNAVAILABLE",
            "The service is currently unavailable.",
            false,
        ),
        (
            403,
            "PERMISSION_DENIED",
            "Permission 'cloudkms.cryptoKeyVersions.useToDecrypt' denied",
            false,
        ),
        (
            400,
            "FAILED_PRECONDITION",
            "CryptoKeyVersion is not enabled, current state is: DISABLED.",
            false,
        ),
        (
            400,
            "INVALID_ARGUMENT",
            "Decryption failed: verify that 'ciphertext' and 'additional_authenticated_data' are correct.",
            true,
        ),
        (418, "TEAPOT", "short and stout", false),
        (429, "RESOURCE_EXHAUSTED", "Quota exceeded", false),
    ];
    for (status, code, message, invalid) in cases {
        let mocks = MockServer::start().await;
        mount_token(&mocks, 1).await;
        Mock::given(method("POST"))
            .and(path(decrypt_path()))
            .respond_with(kms_error(status, code, message))
            .expect(1)
            .mount(&mocks)
            .await;
        let err = kek(&mocks)
            .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION)
            .await
            .unwrap_err();
        let text = err.to_string();
        assert_eq!(
            matches!(err, KekError::Invalid(_)),
            invalid,
            "{status} {code}: {text}"
        );
        assert!(text.contains(code), "the answer's status is named: {text}");
        assert!(
            !text.contains(BEARER),
            "the bearer reached an error string: {text}"
        );
    }

    let mocks = MockServer::start().await;
    mount_token(&mocks, 1).await;
    Mock::given(method("POST"))
        .and(path(decrypt_path()))
        .respond_with(ResponseTemplate::new(400).set_body_string("<html>nope</html>"))
        .mount(&mocks)
        .await;
    let err = kek(&mocks)
        .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION)
        .await
        .unwrap_err();
    assert!(matches!(err, KekError::Unavailable(_)), "{err}");
}

/// A metadata server that will not issue a token holds, and KMS is never asked.
#[tokio::test]
async fn no_bearer_is_a_hold_and_kms_is_not_asked() {
    let mocks = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(TOKEN_PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&mocks)
        .await;
    let dek = Vault::new().new_dek().unwrap();
    let err = kek(&mocks).wrap(&http(), &dek, &ROW).await.unwrap_err();
    assert!(matches!(err, KekError::Unavailable(_)), "{err}");
    assert!(
        mocks
            .received_requests()
            .await
            .unwrap()
            .iter()
            .all(|r| r.method == "GET"),
        "kms was asked with no bearer"
    );
}

/// A 401 means the bearer is dead: it is dropped, and the next call fetches a fresh one.
#[tokio::test]
async fn a_refused_bearer_is_dropped_and_refetched() {
    let mocks = MockServer::start().await;
    mount_token(&mocks, 2).await;
    Mock::given(method("POST"))
        .and(path(decrypt_path()))
        .respond_with(kms_error(
            401,
            "UNAUTHENTICATED",
            "Request had invalid authentication credentials.",
        ))
        .up_to_n_times(1)
        .mount(&mocks)
        .await;
    Mock::given(method("POST"))
        .and(path(decrypt_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "plaintext": STANDARD.encode(DEK),
        })))
        .mount(&mocks)
        .await;
    let kek = kek(&mocks);

    let err = kek
        .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION)
        .await
        .unwrap_err();
    assert!(matches!(err, KekError::Unavailable(_)), "{err}");
    let dek = kek
        .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION)
        .await
        .unwrap();
    assert_eq!(dek.as_bytes(), &DEK);
}

/// A plaintext that is not a 32-byte key is a broken row, not an outage.
#[tokio::test]
async fn a_plaintext_of_the_wrong_length_is_invalid() {
    let mocks = MockServer::start().await;
    mount_token(&mocks, 1).await;
    Mock::given(method("POST"))
        .and(path(decrypt_path()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "plaintext": STANDARD.encode([1u8; 16]),
        })))
        .mount(&mocks)
        .await;
    let err = kek(&mocks)
        .unwrap_dek(&http(), &WRAPPED, &ROW, KEK_VERSION)
        .await
        .unwrap_err();
    assert!(matches!(err, KekError::Invalid(_)), "{err}");
}
