//! The id token an external OpenID Connect provider answers a code exchange
//! with, verified before anything in it is believed (OIDC Core § 3.1.3.7):
//! RS256 under a key the provider publishes, from its issuer, addressed to
//! this client, and unexpired.
//!
//! The only token this system checks by signature. The session's own bearer
//! is an opaque secret auth mints and looks up in its tables
//! ([`crate::person_token`]), and nothing of the provider's is relayed after
//! the exchange, so no key of ours signs anything.

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;

use crate::error::{AuthError, TelmoniError};

use super::jwks::{JwksCache, KeySetUrl};

/// How far past `exp` a token is still taken, for clock skew between us and
/// the provider. The exchange spends the token the moment it arrives, so
/// this is slack for clocks, never a second lifetime.
const EXP_LEEWAY_SECS: i64 = 30;

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

/// `aud` as either spelling the standard allows: one audience, or several.
#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn names(&self, client_id: &str) -> bool {
        match self {
            Self::One(aud) => aud == client_id,
            Self::Many(auds) => auds.iter().any(|aud| aud == client_id),
        }
    }
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    exp: i64,
    #[serde(default)]
    nbf: Option<i64>,
    #[serde(default)]
    aud: Option<Audience>,
}

/// Verifies one provider's id tokens. One per configured provider, built at
/// boot; the key set is fetched when the first token needs it.
pub struct IdTokenVerifier {
    issuer: String,
    client_id: String,
    jwks: JwksCache,
}

impl std::fmt::Debug for IdTokenVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IdTokenVerifier")
            .field("issuer", &self.issuer)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl IdTokenVerifier {
    /// `key_set` is where the provider publishes its signing keys; `issuer`
    /// what a token must carry in `iss`; `client_id` what it must name in
    /// `aud`.
    #[must_use]
    pub fn new(http: reqwest::Client, key_set: KeySetUrl, client_id: &str, issuer: &str) -> Self {
        Self {
            issuer: issuer.to_owned(),
            client_id: client_id.to_owned(),
            jwks: JwksCache::new(http, key_set),
        }
    }

    /// Verify one id token: RS256 under a key the provider publishes, from
    /// its issuer, for our client, unexpired, naming a subject. What the
    /// token then says about the person is the caller's to read.
    pub async fn verify(&self, token: &str) -> Result<(), TelmoniError> {
        // The signed bytes are everything before the last dot, exactly as sent.
        let Some((signing_input, signature_b64)) = token.rsplit_once('.') else {
            return Err(refuse(
                "not three dot-separated segments",
                AuthError::InvalidToken,
            ));
        };
        let Some((header_b64, claims_b64)) = signing_input.split_once('.') else {
            return Err(refuse(
                "not three dot-separated segments",
                AuthError::InvalidToken,
            ));
        };
        if claims_b64.contains('.') {
            return Err(refuse(
                "not three dot-separated segments",
                AuthError::InvalidToken,
            ));
        }
        let header: Header = decode_json(header_b64)
            .ok_or_else(|| refuse("header is not readable", AuthError::InvalidToken))?;
        if header.alg != "RS256" {
            return Err(refuse("alg is not RS256", AuthError::InvalidToken));
        }
        let Some(kid) = header.kid.filter(|k| !k.is_empty()) else {
            return Err(refuse("no kid", AuthError::InvalidToken));
        };
        let Ok(signature) = URL_SAFE_NO_PAD.decode(signature_b64) else {
            return Err(refuse(
                "signature is not base64url",
                AuthError::InvalidToken,
            ));
        };

        let Some(key) = self.jwks.key_for(&kid).await? else {
            return Err(refuse(
                "kid is not in the provider's key set",
                AuthError::InvalidToken,
            ));
        };
        if !key.verifies(signing_input.as_bytes(), &signature) {
            return Err(refuse("signature does not verify", AuthError::InvalidToken));
        }

        let claims: Claims = decode_json(claims_b64)
            .ok_or_else(|| refuse("claims are not readable", AuthError::InvalidToken))?;
        if !same_issuer(&claims.iss, &self.issuer) {
            // The received issuer is logged: it is not a secret, and without
            // it a misconfigured issuer refuses every sign-in with nothing to
            // say what the provider sent instead.
            tracing::warn!(received = %claims.iss, expected = %self.issuer,
                "id-token: refused why=\"issuer is not ours\"");
            return Err(AuthError::InvalidToken.into());
        }
        // A token minted for another client of the same provider verifies
        // under the same keys and carries the same issuer; the audience is
        // what tells it apart, and a token that names none is nobody's.
        match &claims.aud {
            Some(aud) if aud.names(&self.client_id) => {}
            Some(_) => return Err(refuse("aud is not ours", AuthError::InvalidToken)),
            None => return Err(refuse("no aud", AuthError::InvalidToken)),
        }
        let now = chrono::Utc::now().timestamp();
        if claims.exp.saturating_add(EXP_LEEWAY_SECS) <= now {
            return Err(refuse("expired", AuthError::TokenExpired));
        }
        if claims
            .nbf
            .is_some_and(|nbf| nbf.saturating_sub(EXP_LEEWAY_SECS) > now)
        {
            return Err(refuse("not yet valid", AuthError::InvalidToken));
        }
        if claims.sub.trim().is_empty() {
            return Err(refuse("no sub", AuthError::InvalidToken));
        }
        Ok(())
    }
}

/// Every refusal is one of a few wire answers; the distinction lives in the log.
fn refuse(why: &'static str, error: AuthError) -> TelmoniError {
    tracing::warn!(why, "id-token: refused");
    error.into()
}

/// Issuers compare equal ignoring a trailing slash: an issuer typed either
/// way must not refuse every sign-in over it.
fn same_issuer(received: &str, expected: &str) -> bool {
    received.trim_end_matches('/') == expected.trim_end_matches('/')
}

fn decode_json<T: serde::de::DeserializeOwned>(segment: &str) -> Option<T> {
    let bytes = URL_SAFE_NO_PAD.decode(segment).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::id_token::{
        PROVIDER_KID, TEST_CLIENT_ID, TEST_PROVIDER_ISSUER, id_token_under, provider_id_token,
        provider_jwks_json,
    };
    use serde_json::json;

    fn claims(sub: &str, exp: i64) -> serde_json::Value {
        json!({ "iss": TEST_PROVIDER_ISSUER, "sub": sub, "aud": TEST_CLIENT_ID, "exp": exp, "iat": exp - 3600 })
    }

    fn live(sub: &str) -> serde_json::Value {
        claims(sub, chrono::Utc::now().timestamp() + 600)
    }

    /// A key-set stub at `/jwks` serving the provider's keys, and a verifier
    /// for the provider it stands in for.
    async fn provider_stub() -> (wiremock::MockServer, IdTokenVerifier) {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/jwks"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<serde_json::Value>(&provider_jwks_json()).unwrap(),
            ))
            .mount(&server)
            .await;
        let v = IdTokenVerifier::new(
            reqwest::Client::new(),
            KeySetUrl::Fixed(format!("{}/jwks", server.uri())),
            TEST_CLIENT_ID,
            TEST_PROVIDER_ISSUER,
        );
        (server, v)
    }

    #[test]
    fn issuers_compare_ignoring_a_trailing_slash_and_nothing_else() {
        assert!(same_issuer("https://a.example/x/", "https://a.example/x"));
        assert!(same_issuer("https://a.example/x", "https://a.example/x/"));
        assert!(!same_issuer(
            "https://a.example/x.evil",
            "https://a.example/x"
        ));
        assert!(!same_issuer("https://a.example", "https://a.example/x"));
    }

    #[tokio::test]
    async fn a_provider_id_token_verifies() {
        let (_server, v) = provider_stub().await;
        v.verify(&provider_id_token(live("user_1"))).await.unwrap();
        let mut slashed = live("user_1");
        slashed["iss"] = json!(format!("{TEST_PROVIDER_ISSUER}/"));
        v.verify(&provider_id_token(slashed)).await.unwrap();
    }

    #[tokio::test]
    async fn a_tampered_signature_is_refused() {
        let (_server, v) = provider_stub().await;
        let token = provider_id_token(live("user_1"));
        let (head, sig) = token.rsplit_once('.').unwrap();
        let mut bytes = URL_SAFE_NO_PAD.decode(sig).unwrap();
        bytes[0] ^= 0x01;
        let forged = format!("{head}.{}", URL_SAFE_NO_PAD.encode(bytes));
        let err = v.verify(&forged).await.unwrap_err();
        assert_eq!(err.to_problem_details().status, 401);
    }

    #[tokio::test]
    async fn a_claim_edit_breaks_the_signature() {
        let (_server, v) = provider_stub().await;
        let token = provider_id_token(live("user_1"));
        let mut parts: Vec<&str> = token.split('.').collect();
        let edited = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&live("user_2")).unwrap());
        parts[1] = &edited;
        assert!(v.verify(&parts.join(".")).await.is_err());
    }

    #[tokio::test]
    async fn alg_none_and_hs256_are_refused_before_any_key_is_consulted() {
        let (_server, v) = provider_stub().await;
        for alg in ["none", "HS256"] {
            let header = URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&json!({ "alg": alg, "kid": PROVIDER_KID })).unwrap());
            let body = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&live("user_1")).unwrap());
            let token = format!("{header}.{body}.{}", URL_SAFE_NO_PAD.encode(b"x"));
            assert!(v.verify(&token).await.is_err(), "{alg} accepted");
        }
    }

    #[tokio::test]
    async fn expiry_has_thirty_seconds_of_leeway_and_no_more() {
        let (_server, v) = provider_stub().await;
        let now = chrono::Utc::now().timestamp();
        assert!(
            v.verify(&provider_id_token(claims("user_1", now - 10)))
                .await
                .is_ok(),
            "ten seconds past exp is skew, not expiry"
        );
        let err = v
            .verify(&provider_id_token(claims("user_1", now - 60)))
            .await
            .unwrap_err();
        assert_eq!(
            err.to_problem_details().type_uri,
            "/errors/auth/token-expired"
        );
    }

    /// The session is ours, opened after the exchange, so the provider's own
    /// session id is nothing this checks: a provider that names none in its
    /// id tokens signs people in like one that does.
    #[tokio::test]
    async fn an_id_token_needs_no_session_id() {
        let (_server, v) = provider_stub().await;
        let mut named = live("user_1");
        named["sid"] = json!("session_1");
        assert!(v.verify(&provider_id_token(named)).await.is_ok());
        assert!(v.verify(&provider_id_token(live("user_1"))).await.is_ok());
    }

    #[tokio::test]
    async fn a_token_before_its_nbf_is_refused_with_the_same_leeway_as_exp() {
        let (_server, v) = provider_stub().await;
        let now = chrono::Utc::now().timestamp();
        let mut c = claims("user_1", now + 600);
        c["nbf"] = json!(now + 120);
        let err = v.verify(&provider_id_token(c)).await.unwrap_err();
        assert_eq!(
            err.to_problem_details().type_uri,
            "/errors/auth/invalid-token"
        );
        let mut c = claims("user_1", now + 600);
        c["nbf"] = json!(now + 10);
        assert!(
            v.verify(&provider_id_token(c)).await.is_ok(),
            "ten seconds early is skew, not a future token"
        );
        let mut c = claims("user_1", now + 600);
        c["nbf"] = json!(now - 10);
        assert!(v.verify(&provider_id_token(c)).await.is_ok());
    }

    #[tokio::test]
    async fn a_foreign_issuer_is_refused() {
        let (_server, v) = provider_stub().await;
        let mut c = live("user_1");
        c["iss"] = json!("https://someone-else.example/");
        assert!(v.verify(&provider_id_token(c)).await.is_err());
    }

    /// An id token names its client in `aud`, as one string or a list; one
    /// minted for another client of the same provider is refused, and so is
    /// one that names no audience at all.
    #[tokio::test]
    async fn aud_is_required_and_held_to_our_client_id_in_either_spelling() {
        let (_server, v) = provider_stub().await;
        let mut many = live("user_1");
        many["aud"] = json!(["another_client", TEST_CLIENT_ID]);
        assert!(v.verify(&provider_id_token(many)).await.is_ok());
        for refused in [
            json!("another_client"),
            json!(["another_client"]),
            serde_json::Value::Null,
        ] {
            let mut theirs = live("user_1");
            if refused.is_null() {
                theirs.as_object_mut().unwrap().remove("aud");
            } else {
                theirs["aud"] = refused.clone();
            }
            let err = v.verify(&provider_id_token(theirs)).await.unwrap_err();
            assert_eq!(
                err.to_problem_details().type_uri,
                "/errors/auth/invalid-token",
                "{refused}"
            );
        }
    }

    #[tokio::test]
    async fn a_token_that_names_nobody_is_refused() {
        let (_server, v) = provider_stub().await;
        assert!(v.verify(&provider_id_token(live(""))).await.is_err());
    }

    #[tokio::test]
    async fn an_unknown_kid_is_refetched_once_a_minute_and_refused_meanwhile() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/jwks"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<serde_json::Value>(&provider_jwks_json()).unwrap(),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let v = IdTokenVerifier::new(
            reqwest::Client::new(),
            KeySetUrl::Fixed(format!("{}/jwks", server.uri())),
            TEST_CLIENT_ID,
            TEST_PROVIDER_ISSUER,
        );
        // The provider's kid IS in the served set: the first miss fetches and finds it.
        assert!(v.verify(&provider_id_token(live("user_1"))).await.is_ok());
        // A kid the set lacks does not fetch again within the minute.
        assert!(
            v.verify(&id_token_under("nope", live("user_1")))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn an_unreadable_key_set_with_nothing_cached_is_a_503_not_a_401() {
        let v = IdTokenVerifier::new(
            reqwest::Client::new(),
            KeySetUrl::Fixed("http://127.0.0.1:1/jwks".into()),
            TEST_CLIENT_ID,
            TEST_PROVIDER_ISSUER,
        );
        let err = v
            .verify(&provider_id_token(live("user_1")))
            .await
            .unwrap_err();
        assert_eq!(err.to_problem_details().status, 503);
    }

    /// The key set may be the `jwks_uri` of a discovery document rather
    /// than a URL known at boot; the fetch reads the document first.
    #[tokio::test]
    async fn the_key_set_can_be_discovered() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(
                "/.well-known/openid-configuration",
            ))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(json!({
                "issuer": server.uri(),
                "authorization_endpoint": format!("{}/authorize", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
                "jwks_uri": format!("{}/keys", server.uri()),
            })))
            .expect(1)
            .mount(&server)
            .await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/keys"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<serde_json::Value>(&provider_jwks_json()).unwrap(),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let discovery = std::sync::Arc::new(crate::oidc::Discovery::new(
            reqwest::Client::new(),
            &server.uri(),
        ));
        let v = IdTokenVerifier::new(
            reqwest::Client::new(),
            KeySetUrl::Discovered(discovery),
            TEST_CLIENT_ID,
            &server.uri(),
        );
        let now = chrono::Utc::now().timestamp();
        let token = provider_id_token(
            json!({ "iss": server.uri(), "sub": "user_1", "aud": TEST_CLIENT_ID, "exp": now + 600, "iat": now }),
        );
        assert!(v.verify(&token).await.is_ok());
    }
}
