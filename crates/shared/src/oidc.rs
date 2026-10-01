//! OpenID Connect, as a relying party: discovery, the provider's key set
//! ([`jwks`]), and the id token a code exchange answers ([`id_token`]).
//!
//! Discovery reads the provider's endpoints from
//! `{issuer}/.well-known/openid-configuration` the first time anything needs
//! one and holds them for the life of the process. Lazy rather than fetched
//! at boot, so a provider that is briefly unreachable when a pod starts is a
//! warning and a retry, not a crashloop, and a laptop or a test that never
//! signs anybody in needs no provider at all.

pub mod id_token;
pub mod jwks;

pub use id_token::IdTokenVerifier;
pub use jwks::KeySetUrl;

use tokio::sync::OnceCell;

use crate::error::{AuthError, TelmoniError};

/// The endpoints a relying party uses, as the discovery document names them
/// (OpenID Connect Discovery 1.0 § 3). Only the two the sign-in cannot do
/// without are required; the rest are offered by most providers and each
/// caller says what it does when one is missing.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct Endpoints {
    /// The issuer the document claims, which must be the one it was fetched
    /// for (§ 4.3): a document that names another is somebody else's.
    pub issuer: String,
    /// Where the browser is sent to sign in.
    pub authorization_endpoint: String,
    /// Where a code, a refresh token or a device code is exchanged.
    pub token_endpoint: String,
    /// The signing keys every token from this issuer is verified under.
    pub jwks_uri: String,
    /// The device authorization grant (RFC 8628), which the CLI signs in with.
    #[serde(default)]
    pub device_authorization_endpoint: Option<String>,
    /// RP-initiated logout: the browser page that ends the provider's session.
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
    /// Token revocation (RFC 7009).
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    /// How the token endpoint takes the client's credentials. Absent means
    /// `client_secret_basic`, the one method every server must support
    /// (RFC 6749 § 2.3.1).
    #[serde(default)]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
}

/// The discovery document for one issuer, fetched once.
pub struct Discovery {
    http: reqwest::Client,
    issuer: String,
    cell: OnceCell<Endpoints>,
}

impl std::fmt::Debug for Discovery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Discovery")
            .field("issuer", &self.issuer)
            .field("resolved", &self.cell.initialized())
            .finish()
    }
}

impl Discovery {
    /// Discovery against `issuer`, resolved on first use.
    #[must_use]
    pub fn new(http: reqwest::Client, issuer: &str) -> Self {
        Self {
            http,
            issuer: issuer.trim_end_matches('/').to_owned(),
            cell: OnceCell::new(),
        }
    }

    /// Endpoints known ahead of time, for a test or a provider whose document
    /// is not where the standard puts it. Never fetches.
    #[must_use]
    pub fn fixed(endpoints: Endpoints) -> Self {
        Self {
            http: reqwest::Client::new(),
            issuer: endpoints.issuer.trim_end_matches('/').to_owned(),
            cell: OnceCell::new_with(Some(endpoints)),
        }
    }

    /// The configured issuer, trailing slash removed.
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The document's URL (§ 4.1).
    #[must_use]
    pub fn document_url(&self) -> String {
        format!("{}/.well-known/openid-configuration", self.issuer)
    }

    /// The endpoints, fetching the document if this is the first ask. A
    /// document that cannot be read is a 503: the provider is what is
    /// unavailable, and nothing about the request is wrong.
    pub async fn endpoints(&self) -> Result<&Endpoints, TelmoniError> {
        self.cell
            .get_or_try_init(|| async {
                self.fetch().await.map_err(|e| {
                    tracing::error!(
                        error = %e,
                        url = %self.document_url(),
                        "identity provider discovery document unreadable"
                    );
                    AuthError::IdentityUnavailable.into()
                })
            })
            .await
    }

    /// Fetch now, for a boot-time warm-up. A failure is the caller's to log
    /// and shrug at; the next ask fetches again.
    pub async fn warm(&self) -> Result<(), String> {
        self.endpoints()
            .await
            .map(|_| ())
            .map_err(|_| format!("discovery document unreadable at {}", self.document_url()))
    }

    async fn fetch(&self) -> Result<Endpoints, String> {
        let resp = self
            .http
            .get(self.document_url())
            .send()
            .await
            .map_err(|e| format!("unreachable: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("answered {}", resp.status()));
        }
        let endpoints: Endpoints = resp
            .json()
            .await
            .map_err(|e| format!("document is not a discovery document: {e}"))?;
        if endpoints.issuer.trim_end_matches('/') != self.issuer {
            return Err(format!(
                "document names issuer {}, not the configured {}",
                endpoints.issuer, self.issuer
            ));
        }
        Ok(endpoints)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn document(issuer: &str) -> serde_json::Value {
        serde_json::json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
        })
    }

    /// One fetch serves every ask, and a trailing slash on the configured
    /// issuer is not a mismatch.
    #[tokio::test]
    async fn the_document_is_fetched_once_and_a_trailing_slash_is_forgiven() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(document(&server.uri())))
            .expect(1)
            .mount(&server)
            .await;
        let discovery = Discovery::new(reqwest::Client::new(), &format!("{}/", server.uri()));
        let first = discovery.endpoints().await.unwrap().token_endpoint.clone();
        let second = discovery.endpoints().await.unwrap().token_endpoint.clone();
        assert_eq!(first, format!("{}/token", server.uri()));
        assert_eq!(first, second);
        assert!(
            discovery
                .endpoints()
                .await
                .unwrap()
                .device_authorization_endpoint
                .is_none()
        );
    }

    /// A document naming another issuer is refused: a misdirected issuer URL
    /// must not quietly trust whoever answers there.
    #[tokio::test]
    async fn a_document_for_another_issuer_is_refused() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(document("https://someone-else.example")),
            )
            .mount(&server)
            .await;
        let discovery = Discovery::new(reqwest::Client::new(), &server.uri());
        let err = discovery.endpoints().await.unwrap_err();
        assert_eq!(err.to_problem_details().status, 503);
    }

    /// Unreachable is a 503 and a retry, never a cached failure.
    #[tokio::test]
    async fn an_unreachable_provider_is_a_503_and_is_asked_again() {
        let discovery = Discovery::new(reqwest::Client::new(), "http://127.0.0.1:1");
        assert_eq!(
            discovery
                .endpoints()
                .await
                .unwrap_err()
                .to_problem_details()
                .status,
            503
        );
        assert!(discovery.warm().await.is_err());
        assert!(!discovery.cell.initialized());
    }

    #[tokio::test]
    async fn fixed_endpoints_never_fetch() {
        let discovery = Discovery::fixed(Endpoints {
            issuer: "http://127.0.0.1:1/".into(),
            authorization_endpoint: "http://127.0.0.1:1/authorize".into(),
            token_endpoint: "http://127.0.0.1:1/token".into(),
            jwks_uri: "http://127.0.0.1:1/jwks".into(),
            device_authorization_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
            token_endpoint_auth_methods_supported: None,
        });
        assert_eq!(discovery.issuer(), "http://127.0.0.1:1");
        assert!(discovery.endpoints().await.is_ok());
    }
}
