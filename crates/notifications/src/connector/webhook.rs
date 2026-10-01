//! The signed webhook: a customer's own HTTPS endpoint, told about every
//! notice in JSON it can verify.
//!
//! ⚠ The one connector where the person supplies the URL, so the one with an
//! SSRF surface; `telmoni_shared::net_guard` closes it at registration, at dial
//! and at connect. `https` only.
//!
//! **The secret is ours, shown once.** `Telmoni-Signature: t=<unix>,v1=<hex>`,
//! HMAC-SHA256 over `"{t}.{body}"` — Stripe's shape, so an integrator already
//! knows it. `Telmoni-Delivery-Id` is stable across redeliveries, the
//! receiver's dedup key. The secret goes only into the MAC.
//!
//! **During a rotation's overlap there are two `v1`s**, the new secret's first
//! and then the prior one's, as Stripe sends during a roll. A receiver accepts
//! the delivery when ANY `v1` verifies, so it keeps working whichever secret it
//! holds while it is being moved.
//!
//! **Almost no answer is terminal**: a 404 mid-deploy comes back. The exception
//! is `410 Gone`; every other dead endpoint is ended by the breaker.

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use reqwest::Url;
use ring::rand::SecureRandom as _;
use serde_json::{Value, json};
use sha2::Sha256;
use telmoni_shared::net_guard::{Egress, HostRejection, host_is_dialable};
use telmoni_shared::{Redacted, TelmoniError};

use super::{
    Connector, DeliveryError, Event, Provider, Terminal, parse_retry_after, refused, short_body,
};

type HmacSha256 = Hmac<Sha256>;

/// The header a receiver reads for the signature.
pub const SIGNATURE_HEADER: &str = "telmoni-signature";

/// The delivery row's id, stable across every redelivery of that row.
pub const DELIVERY_ID_HEADER: &str = "telmoni-delivery-id";

/// What the row's `scopes` column records: the scheme a receiver verifies.
pub const SCHEME: &str = "telmoni-signature-v1";

/// Consecutive failed POSTs before the connection is `errored`. Ten, because a
/// notice spends at most five: one bad hour cannot trip it, two lost notices can.
pub const BREAKER_AFTER: i32 = 10;

/// The longest endpoint URL accepted. A URL is an address, not a payload.
pub const MAX_URL_LEN: usize = 2048;

/// The prefix a signing secret carries, so one is recognisable in config and
/// greppable in a leak.
const SECRET_PREFIX: &str = "whsec_";

/// How far a receiver should let `t=` stray before refusing a replay: five
/// minutes, Stripe's number. Every attempt is signed when SENT, so a retry
/// after a long backoff still lands inside the window.
pub const REPLAY_TOLERANCE_SECS: i64 = 300;

/// Parse and check a customer-supplied endpoint, or say in a sentence why not;
/// the dialog renders that sentence as it is.
pub fn validate_endpoint_url(raw: &str) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.len() > MAX_URL_LEN {
        return Err(format!(
            "the endpoint URL is too long ({MAX_URL_LEN} characters at most)"
        ));
    }
    let mut url = Url::parse(raw).map_err(|_| "the endpoint must be a URL".to_owned())?;
    if url.scheme() != "https" {
        return Err("the endpoint must be https".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("the endpoint must not carry a username or password".into());
    }
    host_is_dialable(&url).map_err(|why| match why {
        HostRejection::Malformed | HostRejection::Missing => {
            "the endpoint must name a host".to_owned()
        }
        HostRejection::Local | HostRejection::NotGlobal => format!("the endpoint {why}"),
    })?;
    url.set_fragment(None);
    Ok(url)
}

/// The one thing about an endpoint the page shows: its host and port. The path
/// may carry a token, so it stays in the vault.
#[must_use]
pub fn host_label(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

/// SHA-256 of the normalised URL. The URL is sealed, so this is what the
/// unique index holds.
#[must_use]
pub fn fingerprint(url: &Url) -> String {
    telmoni_shared::digest::sha256_hex(url.as_str().as_bytes())
}

/// Thirty-two bytes from the OS, hex, behind the prefix.
pub fn mint_signing_secret() -> Result<Redacted, TelmoniError> {
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| TelmoniError::Internal("no randomness for a signing secret".into()))?;
    Ok(Redacted::from(format!(
        "{SECRET_PREFIX}{}",
        hex::encode(bytes)
    )))
}

/// The `t=<unix>,v1=<hex>` signature for `body` at `timestamp`.
#[must_use]
pub fn sign(secret: &str, body: &[u8], timestamp: i64) -> String {
    sign_all(&[secret], body, timestamp)
}

/// One `v1` per secret, in the order given, behind one `t=`: the header a
/// rotation's overlap sends.
#[must_use]
pub fn sign_all(secrets: &[&str], body: &[u8], timestamp: i64) -> String {
    let mut header = format!("t={timestamp}");
    for secret in secrets {
        let Ok(mut mac) = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes()) else {
            continue;
        };
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b".");
        mac.update(body);
        header.push_str(",v1=");
        header.push_str(&hex::encode(mac.finalize().into_bytes()));
    }
    header
}

/// The body a receiver gets: the notice as emitted, plus the delivery id and
/// send time, and no fact the event did not carry.
#[must_use]
pub fn render(event: &Event<'_>, sent_at: chrono::DateTime<chrono::Utc>) -> Value {
    json!({
        "id": event.id,
        "kind": event.kind.to_string(),
        "title": event.title,
        "body": event.body,
        "sent_at": sent_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    })
}

/// Every non-2xx but `410` is transient, paced by `Retry-After` when sent.
#[must_use]
pub fn classify(
    status: reqwest::StatusCode,
    body: &str,
    retry_after: Option<i64>,
) -> DeliveryError {
    let message = if body.is_empty() {
        format!("endpoint returned {status}")
    } else {
        format!("endpoint returned {status}: {body}")
    };
    if status == reqwest::StatusCode::GONE {
        return DeliveryError::terminal(Terminal::BadTarget, message);
    }
    DeliveryError::transient_after(message, retry_after)
}

/// The connector. It holds nothing; the only credential is the row's own.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebhookConnector;

#[async_trait]
impl Connector for WebhookConnector {
    fn provider(&self) -> Provider {
        Provider::Webhook
    }

    fn delivers_with_token(&self) -> bool {
        true
    }

    fn consecutive_failure_limit(&self) -> Option<i32> {
        Some(BREAKER_AFTER)
    }

    async fn deliver(
        &self,
        http: &Egress,
        target: &Redacted,
        token: Option<&Redacted>,
        prior: Option<&Redacted>,
        event: &Event<'_>,
    ) -> Result<u16, DeliveryError> {
        let Some(secret) = token else {
            return Err(DeliveryError::terminal(
                Terminal::OurBug,
                "the webhook row holds no signing secret",
            ));
        };
        let now = chrono::Utc::now();
        let body = serde_json::to_vec(&render(event, now)).map_err(|e| {
            DeliveryError::terminal(
                Terminal::OurBug,
                format!("payload would not serialise: {e}"),
            )
        })?;
        let signature = match prior {
            Some(prior) => sign_all(&[secret.expose(), prior.expose()], &body, now.timestamp()),
            None => sign(secret.expose(), &body, now.timestamp()),
        };
        let resp = http
            .post(target.expose())
            .map_err(refused)?
            .header(SIGNATURE_HEADER, signature)
            .header(DELIVERY_ID_HEADER, event.id.to_string())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                DeliveryError::transient(format!("endpoint POST failed: {}", e.without_url()))
            })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(status.as_u16());
        }
        let retry_after = parse_retry_after(resp.headers());
        let body = short_body(resp).await;
        Err(classify(status, &body, retry_after).answered(status))
    }

    async fn tear_down(
        &self,
        _http: &Egress,
        _target: &Redacted,
        _token: Option<&Redacted>,
    ) -> Result<(), TelmoniError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference verifier: what a receiver writes, proving it can validate
    /// what we sign. Any `v1` that verifies is enough.
    fn verify(secret: &str, header: &str, body: &[u8]) -> bool {
        let mut t: Option<i64> = None;
        let mut candidates: Vec<Vec<u8>> = Vec::new();
        for part in header.split(',') {
            match part.split_once('=') {
                Some(("t", v)) => t = v.parse().ok(),
                Some(("v1", v)) => candidates.extend(hex::decode(v).ok()),
                _ => {}
            }
        }
        let Some(t) = t else {
            return false;
        };
        candidates.iter().any(|candidate| {
            let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes()).unwrap();
            mac.update(t.to_string().as_bytes());
            mac.update(b".");
            mac.update(body);
            mac.verify_slice(candidate).is_ok()
        })
    }

    /// ⚠ **The published golden vector.** The same inputs appear on the docs
    /// site and in `web/lib/webhook-signature.test.ts`, and the hex was computed
    /// independently in Python. Breaking this breaks every receiver written
    /// against the page.
    #[test]
    fn the_published_golden_vector_verifies() {
        const SECRET: &str =
            "whsec_0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        const BODY: &[u8] = br#"{"id":"019a0b8c-3f2e-7d41-9c5a-6e2b1f0d8a47","kind":"member_added","title":"Sam joined","body":"Sam accepted your invitation.","sent_at":"2027-01-15T12:00:00Z"}"#;
        const T: i64 = 1_800_000_000;
        const EXPECTED: &str =
            "t=1800000000,v1=396556d0e74d35a762e7d75d387e1a7dcb4c728ffcc9bc857830a03f13dd740f";
        assert_eq!(sign(SECRET, BODY, T), EXPECTED);
        assert!(verify(SECRET, EXPECTED, BODY));
    }

    #[test]
    fn a_signature_round_trips_and_a_tampered_body_or_wrong_secret_fails() {
        let body = br#"{"kind":"member_added"}"#;
        let header = sign("whsec_project_secret", body, 1_800_000_000);
        assert!(header.starts_with("t=1800000000,v1="));
        assert!(verify("whsec_project_secret", &header, body));
        assert!(!verify("whsec_project_secret", &header, b"tampered"));
        assert!(!verify("whsec_other", &header, body));
    }

    /// A rotation's overlap: one header that each secret verifies on its own,
    /// the new one first, so a receiver mid-move keeps accepting deliveries.
    #[test]
    fn an_overlap_header_verifies_under_either_secret_and_no_other() {
        let body = br#"{"kind":"member_added"}"#;
        let header = sign_all(&["whsec_new", "whsec_prior"], body, 1_800_000_000);
        assert_eq!(header.matches(",v1=").count(), 2);
        assert!(
            header.starts_with(&sign("whsec_new", body, 1_800_000_000)),
            "the new secret's signature comes first"
        );
        assert!(verify("whsec_new", &header, body));
        assert!(verify("whsec_prior", &header, body));
        assert!(!verify("whsec_other", &header, body));
        assert!(!verify("whsec_new", &header, b"tampered"));
    }

    #[test]
    fn endpoints_are_https_only_never_private_and_never_carry_credentials() {
        assert!(validate_endpoint_url("https://hooks.example.com/x").is_ok());
        assert!(validate_endpoint_url("  https://hooks.example.com/x#frag ").is_ok());
        assert!(validate_endpoint_url("https://203.0.113.9/hook").is_ok());
        for (bad, why) in [
            ("http://hooks.example.com/x", "must be https"),
            (
                "https://user:pw@hooks.example.com/x",
                "username or password",
            ),
            ("https://localhost/hook", "local address"),
            ("https://127.0.0.1/hook", "private or local address"),
            ("https://10.0.0.9/hook", "private or local address"),
            ("https://169.254.169.254/latest", "private or local address"),
            ("https://metadata.google.internal/x", "local address"),
            ("https://[::1]/hook", "private or local address"),
            (
                "https://[::ffff:169.254.169.254]/x",
                "private or local address",
            ),
            ("ftp://example.com/x", "must be https"),
            ("not a url", "must be a URL"),
            ("https://", "must be a URL"),
        ] {
            let err = validate_endpoint_url(bad).expect_err(bad);
            assert!(err.contains(why), "{bad}: {err}");
        }
        let long = format!("https://hooks.example.com/{}", "a".repeat(MAX_URL_LEN));
        assert!(
            validate_endpoint_url(&long)
                .unwrap_err()
                .contains("too long")
        );
    }

    #[test]
    fn the_label_is_the_host_and_the_fingerprint_is_the_whole_url() {
        let a = validate_endpoint_url("https://hooks.example.com/a?x=1").unwrap();
        let b = validate_endpoint_url("https://hooks.example.com/b").unwrap();
        let c = validate_endpoint_url("https://hooks.example.com:8443/a?x=1").unwrap();
        assert_eq!(host_label(&a), "hooks.example.com");
        assert_eq!(host_label(&c), "hooks.example.com:8443");
        assert_eq!(fingerprint(&a).len(), 64);
        assert_ne!(fingerprint(&a), fingerprint(&b));
        assert_ne!(fingerprint(&a), fingerprint(&c));
        assert_eq!(
            fingerprint(&a),
            fingerprint(&validate_endpoint_url("https://hooks.example.com/a?x=1#f").unwrap()),
            "a fragment does not make a second endpoint"
        );
    }

    #[test]
    fn a_minted_secret_is_prefixed_hex_and_never_repeats() {
        let a = mint_signing_secret().unwrap();
        let b = mint_signing_secret().unwrap();
        assert!(a.expose().starts_with(SECRET_PREFIX));
        assert_eq!(a.expose().len(), SECRET_PREFIX.len() + 64);
        assert_ne!(a.expose(), b.expose());
        assert!(
            !format!("{a:?}").contains(a.expose()),
            "Debug must mask the secret"
        );
    }

    #[test]
    fn every_answer_that_is_not_success_is_transient_except_gone() {
        assert!(matches!(
            classify(reqwest::StatusCode::GONE, "", None),
            DeliveryError::Terminal {
                class: Terminal::BadTarget,
                ..
            }
        ));
        for status in [400u16, 401, 403, 404, 422, 429, 500, 503] {
            let e = classify(
                reqwest::StatusCode::from_u16(status).unwrap(),
                "nope",
                Some(7),
            );
            assert!(
                matches!(
                    e,
                    DeliveryError::Transient {
                        retry_after: Some(7),
                        ..
                    }
                ),
                "{status}: {e:?}"
            );
            assert!(e.message().contains(&status.to_string()));
        }
        assert_eq!(
            classify(reqwest::StatusCode::BAD_GATEWAY, "", None).message(),
            "endpoint returned 502 Bad Gateway"
        );
    }
}
