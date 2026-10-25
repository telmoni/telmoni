//! The [`AuthProvider`] this repository ships: any OpenID Connect provider,
//! reached through what the standards give every relying party and nothing
//! a vendor adds.
//!
//! - Discovery (`{issuer}/.well-known/openid-configuration`) names the
//!   endpoints; nothing here spells a path.
//! - The web sign-in is the authorization code flow as a confidential
//!   client. The exchange answers who the person is, read out of the **id
//!   token**, the one artifact every provider signs and addresses to this
//!   client; [`crate::issuer`] then opens the session, so nothing of the
//!   provider's is relayed and nothing is asked of it again until the next
//!   sign-in. The access token is not used: at many providers it is opaque.
//! - Sign-out is RP-initiated logout (`end_session_endpoint`) for the
//!   browser half, naming the session by the id token the exchange
//!   answered.
//! - Password resets, address changes and deleting the person are the
//!   provider's own affair: the trait's defaults refuse them, and the
//!   console hides the settings because this provider reports no sign-in
//!   method (`auth_method: None`).
//!
//! ⚠ **Every error mapping here reads the provider's error CODE, never the
//! status alone.** A bare-404 shortcut in an earlier adapter reported
//! success for revokes that never happened, for months.

use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;

use telmoni_shared::oidc::{Discovery, Endpoints, IdTokenVerifier};
use telmoni_shared::{AuthError, Redacted, TelmoniError, digest::hex_digit};

use crate::provider::{AuthProvider, Authenticated, Binding, Proof, Subject};

/// Percent-encode a query-string value: a space is `%20`, never `+`.
pub(crate) fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            other => {
                out.push('%');
                out.push(hex_digit(other >> 4).to_ascii_uppercase());
                out.push(hex_digit(other).to_ascii_uppercase());
            }
        }
    }
    out
}

/// `endpoint?a=b&c=d`, or `endpoint&a=b…` when the provider's endpoint
/// already carries a query string.
pub(crate) fn with_query(endpoint: &str, params: &[(&str, &str)]) -> String {
    let mut url = String::from(endpoint);
    for (i, (k, v)) in params.iter().enumerate() {
        url.push(if i == 0 && !endpoint.contains('?') {
            '?'
        } else {
            '&'
        });
        url.push_str(k);
        url.push('=');
        url.push_str(&urlencoding(v));
    }
    url
}

/// A token endpoint's success body (RFC 6749 § 5.1, OIDC Core § 3.1.3.3):
/// the id token, and the access token the userinfo endpoint is read with
/// where the deployment asks for it.
#[derive(Deserialize)]
struct TokenBody {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
}

/// A token endpoint's refusal (RFC 6749 § 5.2).
#[derive(Deserialize)]
struct TokenRefusal {
    #[serde(default)]
    error: Option<String>,
}

/// What an id token says about the person (OIDC Core § 5.1), read once the
/// signature, issuer, audience and expiry have been checked — and what the
/// userinfo endpoint says, read the same way (§ 5.3.2).
#[derive(Deserialize)]
struct IdTokenClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default, deserialize_with = "verified")]
    email_verified: Option<bool>,
    #[serde(default)]
    given_name: Option<String>,
    #[serde(default)]
    family_name: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

/// `email_verified` as a boolean, or as the string some userinfo endpoints
/// answer it with; anything else is no verdict.
fn verified<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Bool(b) => Some(b),
        serde_json::Value::String(s) => match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

impl IdTokenClaims {
    /// The userinfo endpoint's claims over the id token's, each where it
    /// gave one; an address and its verdict come from one source, so
    /// neither vouches for the other's address.
    fn merged(self, userinfo: Self) -> Self {
        let same = |a: &str, b: &str| a.trim().to_lowercase() == b.trim().to_lowercase();
        let (email, email_verified) = match userinfo.email {
            Some(email) if self.email.as_deref().is_some_and(|e| same(e, &email)) => {
                (Some(email), userinfo.email_verified.or(self.email_verified))
            }
            Some(email) => (Some(email), userinfo.email_verified),
            None => (self.email, self.email_verified),
        };
        Self {
            sub: self.sub,
            email,
            email_verified,
            given_name: userinfo.given_name.or(self.given_name),
            family_name: userinfo.family_name.or(self.family_name),
            name: userinfo.name.or(self.name),
            nonce: self.nonce,
        }
    }
}

/// What a provider is held to (`AUTH_<PROVIDER>_CHECKS`): PKCE and the
/// nonce, each on unless the deployment names the
/// provider as one that refuses it; `state` always, since the callback is
/// built on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checks {
    /// The code is bound to its challenge, and redeemed with its verifier.
    pub pkce: bool,
    /// The id token carries back the nonce it was sent.
    pub nonce: bool,
}

impl Default for Checks {
    fn default() -> Self {
        Self {
            pkce: true,
            nonce: true,
        }
    }
}

impl std::str::FromStr for Checks {
    type Err = String;

    /// A comma-separated list of `pkce`, `nonce` and `state`, which must
    /// name `state`: `none`, or a list without it, is refused.
    fn from_str(list: &str) -> Result<Self, Self::Err> {
        let (mut checks, mut state) = (
            Self {
                pkce: false,
                nonce: false,
            },
            false,
        );
        for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            match word.to_ascii_lowercase().as_str() {
                "pkce" => checks.pkce = true,
                "nonce" => checks.nonce = true,
                "state" => state = true,
                other => {
                    return Err(format!(
                        "{other:?} is not a check: the list is of pkce, nonce and state"
                    ));
                }
            }
        }
        if !state {
            return Err(
                "the list leaves out state, which the callback is built on and is never dropped"
                    .into(),
            );
        }
        Ok(checks)
    }
}

/// What a deployment says of its OpenID Connect provider: the client it is
/// there, and what each sign-in asks of it.
pub struct Settings {
    /// This client's id at the provider (`AUTH_CUSTOM_CLIENT_ID`).
    pub client_id: String,
    /// Its secret there (`AUTH_CUSTOM_CLIENT_SECRET`).
    pub client_secret: Redacted,
    /// The issuer URL: what files this provider's subjects, and what its id
    /// tokens must carry in `iss`.
    pub issuer: String,
    /// What the sign-in page calls it (`AUTH_CUSTOM_NAME`).
    pub name: String,
    /// The scopes every sign-in asks for (`AUTH_CUSTOM_SCOPE`): the subject,
    /// the address and the name unless set.
    pub scope: String,
    /// Whether the userinfo endpoint is read beside the id token
    /// (`AUTH_CUSTOM_FETCH_USERINFO`), for a provider that keeps the address
    /// or the name out of its tokens.
    pub fetch_userinfo: bool,
    /// What each sign-in is held to (`AUTH_CUSTOM_CHECKS`).
    pub checks: Checks,
}

/// Any OpenID Connect provider, as a confidential client.
pub struct OidcProvider {
    http: reqwest::Client,
    client_id: String,
    client_secret: Redacted,
    /// The issuer URL: what files this provider's subjects, and what its id
    /// tokens must carry in `iss`.
    issuer: String,
    /// What the sign-in page calls it.
    name: String,
    scope: String,
    fetch_userinfo: bool,
    checks: Checks,
    discovery: Arc<Discovery>,
    /// Verifies the id tokens the exchange answers with, under the
    /// provider's published keys.
    verifier: Arc<IdTokenVerifier>,
}

impl OidcProvider {
    /// Build a provider. `verifier` must be built for its issuer and client
    /// id, and normally for `KeySetUrl::Discovered(discovery)`.
    #[must_use]
    pub fn new(
        http: reqwest::Client,
        settings: Settings,
        discovery: Arc<Discovery>,
        verifier: Arc<IdTokenVerifier>,
    ) -> Self {
        Self {
            http,
            client_id: settings.client_id,
            client_secret: settings.client_secret,
            issuer: settings.issuer.trim_end_matches('/').to_owned(),
            name: settings.name,
            scope: settings.scope,
            fetch_userinfo: settings.fetch_userinfo,
            checks: settings.checks,
            discovery,
            verifier,
        }
    }

    async fn endpoints(&self) -> Result<&Endpoints, TelmoniError> {
        self.discovery.endpoints().await
    }

    /// POST a form to `url` as this client, and hand back the status and body
    /// for the caller to read.
    async fn post_form(
        &self,
        url: &str,
        params: &[(&str, &str)],
        op: &str,
    ) -> Result<(reqwest::StatusCode, Vec<u8>), TelmoniError> {
        let auth = client_auth(self.endpoints().await?);
        let mut form: Vec<(&str, &str)> = Vec::with_capacity(params.len() + 2);
        form.extend_from_slice(params);
        let mut request = self.http.post(url).header("accept", "application/json");
        match auth {
            ClientAuth::Basic => {
                // RFC 6749 § 2.3.1: each half form-encoded, then the pair
                // as HTTP Basic. `client_id` still rides in the body, which
                // some providers ask for by name.
                form.push(("client_id", &self.client_id));
                request = request.basic_auth(
                    urlencoding(&self.client_id),
                    Some(urlencoding(self.client_secret.expose())),
                );
            }
            ClientAuth::Post => {
                form.push(("client_id", &self.client_id));
                form.push(("client_secret", self.client_secret.expose()));
            }
        }
        let resp = request
            .form(&form)
            .send()
            .await
            .map_err(|e| TelmoniError::internal(format!("identity provider {op}"), e))?;
        let status = resp.status();
        let body = resp
            .bytes()
            .await
            .map_err(|e| TelmoniError::internal(format!("identity provider {op} body"), e))?;
        Ok((status, body.to_vec()))
    }

    /// A grant at the token endpoint, and who it names.
    async fn token_grant(
        &self,
        params: &[(&str, &str)],
        op: &str,
        nonce: Option<&str>,
    ) -> Result<Authenticated, TelmoniError> {
        let token_endpoint = self.endpoints().await?.token_endpoint.clone();
        let (status, body) = self.post_form(&token_endpoint, params, op).await?;
        if status.is_success() {
            let parsed: TokenBody = serde_json::from_slice(&body)
                .map_err(|e| TelmoniError::internal(format!("identity provider {op} decode"), e))?;
            return self.authenticated_from(parsed, op, nonce).await;
        }
        if status.is_client_error() {
            let code = refusal_code(&body);
            tracing::warn!(
                op,
                status = %status,
                code = code.as_deref().unwrap_or("-"),
                "identity provider rejected an authorization code we just received"
            );
            return Err(AuthError::InvalidToken.into());
        }
        tracing::error!(op, status = %status, "identity provider grant failed upstream");
        Err(TelmoniError::Internal(format!(
            "identity provider {op}: {status}"
        )))
    }

    /// Verify the id token a grant answered with, and read the person out of
    /// it. No id token is a provider that did not honour `openid`; there is
    /// nobody to name.
    async fn authenticated_from(
        &self,
        body: TokenBody,
        op: &str,
        nonce: Option<&str>,
    ) -> Result<Authenticated, TelmoniError> {
        let Some(id_token) = body.id_token.filter(|t| !t.is_empty()) else {
            tracing::error!(
                op,
                "identity provider answered the grant with no id_token; the person cannot be named"
            );
            return Err(AuthError::InvalidToken.into());
        };
        self.verifier.verify(&id_token).await.map_err(|e| {
            tracing::warn!(op, error = %e, "the id_token the provider minted does not verify");
            e
        })?;
        let mut claims: IdTokenClaims = crate::token_claims::payload(&id_token)
            .ok_or_else(|| TelmoniError::Internal("id_token claims unreadable".into()))?;
        // A token minted for another sign-in carries another nonce, or none:
        // the code it came with was not this browser's.
        if let Some(sent) = nonce
            && claims.nonce.as_deref() != Some(sent)
        {
            tracing::warn!(
                op,
                "the id_token's nonce is not the one this sign-in sent; refused"
            );
            return Err(AuthError::InvalidToken.into());
        }
        if self.fetch_userinfo
            && let Some(userinfo) = self.userinfo(body.access_token.as_deref(), op).await
        {
            // Another person's answer is no answer (Core § 5.3.2).
            if userinfo.sub == claims.sub {
                claims = claims.merged(userinfo);
            } else {
                tracing::warn!(
                    op,
                    "the userinfo endpoint named another subject; its answer is dropped"
                );
            }
        }
        // Entra ID sends `name` alone until its optional claims are set.
        let (given_name, family_name) = match (claims.given_name, claims.family_name) {
            (None, None) => (claims.name, None),
            names => names,
        };
        Ok(Authenticated {
            subject: Subject {
                sub: claims.sub,
                email: claims.email,
                email_verified: claims.email_verified.unwrap_or(false),
                given_name,
                family_name,
            },
            id_token: Some(id_token),
            auth_method: None,
        })
    }

    /// What the userinfo endpoint says of the person the access token is
    /// for, or `None`, logged, where it cannot be read: the id token's own
    /// claims then stand alone.
    async fn userinfo(&self, access_token: Option<&str>, op: &str) -> Option<IdTokenClaims> {
        let Some(access_token) = access_token.filter(|t| !t.is_empty()) else {
            tracing::warn!(
                op,
                "the grant answered no access token; the userinfo endpoint is not read"
            );
            return None;
        };
        let endpoint = match self.endpoints().await {
            Ok(endpoints) => endpoints.userinfo_endpoint.clone(),
            Err(_) => None,
        };
        let Some(endpoint) = endpoint else {
            tracing::warn!(op, "the provider's document names no userinfo endpoint");
            return None;
        };
        let answer = self
            .http
            .get(&endpoint)
            .bearer_auth(access_token)
            .header("accept", "application/json")
            .send()
            .await;
        let resp = match answer {
            Ok(resp) if resp.status().is_success() => resp,
            Ok(resp) => {
                tracing::warn!(op, status = %resp.status(), "the userinfo endpoint refused");
                return None;
            }
            Err(e) => {
                tracing::warn!(op, error = %e, "the userinfo endpoint did not answer");
                return None;
            }
        };
        match resp.json::<IdTokenClaims>().await {
            Ok(claims) => Some(claims),
            Err(e) => {
                tracing::warn!(op, error = %e, "the userinfo answer is not claims");
                None
            }
        }
    }
}

/// How this client proves itself at the token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientAuth {
    /// `client_secret_basic`: the default, and the one every server must
    /// support.
    Basic,
    /// `client_secret_post`: the credentials in the form body, for a
    /// provider whose document offers that and not Basic.
    Post,
}

/// Basic unless the provider's document rules it out and offers Post
/// instead. A document naming neither is a provider that supports Basic
/// and did not say so (RFC 8414 § 2 makes Basic the default).
fn client_auth(endpoints: &Endpoints) -> ClientAuth {
    match endpoints.token_endpoint_auth_methods_supported.as_deref() {
        Some(methods)
            if !methods.iter().any(|m| m == "client_secret_basic")
                && methods.iter().any(|m| m == "client_secret_post") =>
        {
            ClientAuth::Post
        }
        _ => ClientAuth::Basic,
    }
}

/// The `error` code in a token endpoint's refusal, when it sent one.
fn refusal_code(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<TokenRefusal>(body)
        .ok()
        .and_then(|r| r.error)
}

#[async_trait]
impl AuthProvider for OidcProvider {
    fn id(&self) -> &str {
        &self.issuer
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn authorize_url(
        &self,
        redirect_uri: &str,
        state: &str,
        binding: Binding<'_>,
        sign_up: bool,
        login_hint: Option<&str>,
    ) -> Result<String, TelmoniError> {
        let endpoints = self.endpoints().await?;
        let mut params = vec![
            ("response_type", "code"),
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("scope", self.scope.as_str()),
            ("state", state),
        ];
        // `prompt=create` is OpenID Connect's own way to ask for the sign-up
        // screen (Prompt Create 1.0), sent only to a provider that lists it:
        // one that does not is told to refuse the request outright.
        if self.checks.pkce {
            params.push(("code_challenge", binding.code_challenge));
            params.push(("code_challenge_method", "S256"));
        }
        if self.checks.nonce {
            params.push(("nonce", binding.nonce));
        }
        let offers_create = endpoints
            .prompt_values_supported
            .as_deref()
            .is_some_and(|values| values.iter().any(|v| v == "create"));
        if sign_up && offers_create {
            params.push(("prompt", "create"));
        }
        if let Some(hint) = login_hint {
            params.push(("login_hint", hint));
        }
        Ok(with_query(&endpoints.authorization_endpoint, &params))
    }

    async fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
        proof: Proof<'_>,
    ) -> Result<Authenticated, TelmoniError> {
        let mut params = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ];
        if self.checks.pkce {
            params.push(("code_verifier", proof.code_verifier));
        }
        let nonce = self.checks.nonce.then_some(proof.nonce);
        self.token_grant(&params, "exchange", nonce).await
    }

    /// RP-Initiated Logout 1.0: `id_token_hint` names the session to end,
    /// `client_id` and `post_logout_redirect_uri` bring the browser back.
    async fn logout_url(&self, id_token: Option<&str>, return_to: &str) -> String {
        let end_session = match self.endpoints().await {
            Ok(endpoints) => endpoints.end_session_endpoint.clone(),
            Err(_) => None,
        };
        match end_session {
            Some(endpoint) => {
                let mut params = vec![("client_id", self.client_id.as_str())];
                if let Some(id_token) = id_token.filter(|t| !t.is_empty()) {
                    params.push(("id_token_hint", id_token));
                }
                params.push(("post_logout_redirect_uri", return_to));
                with_query(&endpoint, &params)
            }
            None => {
                tracing::debug!(
                    "logout_url: the identity provider publishes no end_session_endpoint; signing out locally"
                );
                return_to.to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636's own example pair, and a nonce, as a sign-in carries them.
    const NONCE: &str = "nonce_0123456789abcdef";
    const BINDING: Binding<'static> = Binding {
        code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
        nonce: NONCE,
    };
    const PROOF: Proof<'static> = Proof {
        code_verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
        nonce: NONCE,
    };
    use telmoni_shared::oidc::KeySetUrl;
    use telmoni_shared::test_util::id_token::{
        TEST_CLIENT_ID, provider_id_token, provider_jwks_json,
    };
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// `client_secret_basic` for the stub client (RFC 6749 § 2.3.1).
    fn basic_credentials() -> String {
        use base64::Engine as _;
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD
                .encode(format!("{TEST_CLIENT_ID}:secret_123"))
        )
    }

    fn provider_with(
        base: &str,
        discovery: Arc<Discovery>,
        methods: Option<Vec<String>>,
    ) -> OidcProvider {
        let _ = methods;
        let verifier = Arc::new(IdTokenVerifier::new(
            reqwest::Client::new(),
            KeySetUrl::Discovered(Arc::clone(&discovery)),
            TEST_CLIENT_ID,
            base,
        ));
        OidcProvider::new(
            reqwest::Client::new(),
            Settings {
                client_id: TEST_CLIENT_ID.to_owned(),
                client_secret: Redacted::from("secret_123"),
                issuer: base.to_owned(),
                name: "Okta".to_owned(),
                scope: "openid email profile".to_owned(),
                fetch_userinfo: false,
                checks: Checks::default(),
            },
            discovery,
            verifier,
        )
    }

    /// A provider at a stub: keys at `/jwks`, a token endpoint at `/token`,
    /// and, with extras, a logout page at `/logout`.
    async fn provider_at(server: &MockServer, with_extras: bool) -> OidcProvider {
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::from_str::<serde_json::Value>(&provider_jwks_json()).unwrap(),
            ))
            .mount(server)
            .await;
        let base = server.uri();
        let discovery = Arc::new(Discovery::fixed(Endpoints {
            issuer: base.clone(),
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            jwks_uri: format!("{base}/jwks"),
            device_authorization_endpoint: None,
            end_session_endpoint: with_extras.then(|| format!("{base}/logout")),
            revocation_endpoint: None,
            token_endpoint_auth_methods_supported: None,
            userinfo_endpoint: Some(format!("{base}/userinfo")),
            prompt_values_supported: with_extras.then(|| vec!["login".into(), "create".into()]),
        }));
        provider_with(&base, discovery, None)
    }

    /// An id token the stub provider would mint: signed under the provider
    /// key, for `aud`, naming the provider's own session when `sid` is given.
    fn id_token(server: &MockServer, aud: &str, sid: Option<&str>) -> String {
        let now = chrono::Utc::now().timestamp();
        provider_id_token(serde_json::json!({
            "iss": server.uri(),
            "sub": "user_01TEST",
            "aud": aud,
            "sid": sid,
            "exp": now + 600,
            "iat": now,
            "email": "ada@example.com",
            "email_verified": true,
            "given_name": "Ada",
            "family_name": "Lovelace",
            "nonce": NONCE,
        }))
    }

    fn token_ok(id_token: &str) -> serde_json::Value {
        serde_json::json!({
            "access_token": "opaque_at_1",
            "token_type": "Bearer",
            "expires_in": 300,
            "refresh_token": "rt_2",
            "id_token": id_token,
        })
    }

    #[tokio::test]
    async fn the_provider_is_filed_under_its_issuer_and_shown_by_its_name() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        assert_eq!(p.id(), server.uri().trim_end_matches('/'));
        assert_eq!(p.name(), "Okta");
    }

    #[tokio::test]
    async fn authorize_url_carries_the_standard_parameters() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        let url = p
            .authorize_url("https://app.example/cb", "st8", BINDING, false, None)
            .await
            .unwrap();
        assert!(
            url.starts_with(&format!("{}/authorize?", server.uri())),
            "{url}"
        );
        assert!(url.contains("response_type=code"), "{url}");
        assert!(
            url.contains(&format!("client_id={TEST_CLIENT_ID}")),
            "{url}"
        );
        assert!(
            url.contains("redirect_uri=https%3A%2F%2Fapp.example%2Fcb"),
            "{url}"
        );
        assert!(url.contains("scope=openid%20email%20profile"), "{url}");
        assert!(url.contains("state=st8"), "{url}");
        assert!(!url.contains("prompt="), "{url}");
        assert!(!url.contains("login_hint"), "{url}");
    }

    /// `prompt=create` goes to a provider that lists it, and never to one
    /// that does not, which Prompt Create 1.0 tells to refuse the request.
    #[tokio::test]
    async fn authorize_url_asks_for_signup_with_a_hint_when_told() {
        let server = MockServer::start().await;
        let p = provider_at(&server, true).await;
        let signup = p
            .authorize_url(
                "https://app.example/cb",
                "s",
                BINDING,
                true,
                Some("ada@example.com"),
            )
            .await
            .unwrap();
        assert!(signup.contains("prompt=create"), "{signup}");
        assert!(signup.contains("login_hint=ada%40example.com"), "{signup}");

        let quiet = MockServer::start().await;
        let p = provider_at(&quiet, false).await;
        let signup = p
            .authorize_url("https://app.example/cb", "s", BINDING, true, None)
            .await
            .unwrap();
        assert!(!signup.contains("prompt="), "{signup}");
    }

    /// The code is bound to its sign-in: the challenge, its method and the
    /// nonce ride the authorize URL, the verifier the token request, and an
    /// id token carrying another nonce is refused.
    #[tokio::test]
    async fn the_code_is_bound_to_its_challenge_and_its_token_to_its_nonce() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        let url = p
            .authorize_url("https://app.example/cb", "st8", BINDING, false, None)
            .await
            .unwrap();
        assert!(
            url.contains("code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
            "{url}"
        );
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.contains(&format!("nonce={NONCE}")), "{url}");

        let token = id_token(&server, TEST_CLIENT_ID, None);
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code=code_1"))
            .and(body_string_contains(
                "code_verifier=dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .expect(1)
            .mount(&server)
            .await;
        p.exchange_code("code_1", "https://app.example/cb", PROOF)
            .await
            .expect("bound, and redeemed with its verifier");

        let replayed = id_token_of(
            &server,
            serde_json::json!({ "nonce": "another_sign_in_00000" }),
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code=code_2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&replayed)))
            .mount(&server)
            .await;
        let refused = p
            .exchange_code("code_2", "https://app.example/cb", PROOF)
            .await
            .expect_err("another sign-in's token");
        assert_eq!(refused.to_problem_details().status, 401);
    }

    /// A provider named as one that refuses PKCE and the nonce is sent
    /// neither, and its token is not asked for one.
    #[tokio::test]
    async fn a_provider_held_to_state_alone_is_sent_no_challenge_or_nonce() {
        let server = MockServer::start().await;
        let p = OidcProvider {
            checks: "state".parse().unwrap(),
            ..provider_at(&server, false).await
        };
        let url = p
            .authorize_url("https://app.example/cb", "st8", BINDING, false, None)
            .await
            .unwrap();
        assert!(
            !url.contains("code_challenge") && !url.contains("nonce="),
            "{url}"
        );
        let token = id_token_of(&server, serde_json::json!({ "nonce": null }));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .mount(&server)
            .await;
        p.exchange_code("code_1", "https://app.example/cb", PROOF)
            .await
            .expect("no nonce asked for");
    }

    /// `AUTH_CUSTOM_CHECKS` names pkce, nonce and state, and never leaves
    /// out state.
    #[test]
    fn the_checks_name_pkce_nonce_and_state_and_never_leave_out_state() {
        assert_eq!(
            "pkce,nonce,state".parse::<Checks>().unwrap(),
            Checks::default()
        );
        assert_eq!(
            " State , PKCE ".parse::<Checks>().unwrap(),
            Checks {
                pkce: true,
                nonce: false
            }
        );
        for refused in ["none", "pkce,nonce", "state,sessions", ""] {
            assert!(refused.parse::<Checks>().is_err(), "{refused:?}");
        }
    }

    /// A provider reading userinfo, at a stub that serves its keys.
    async fn reading_userinfo(server: &MockServer) -> OidcProvider {
        let p = provider_at(server, false).await;
        OidcProvider {
            fetch_userinfo: true,
            ..p
        }
    }

    /// An id token of these claims beside the stub's issuer, audience and
    /// clock.
    fn id_token_of(server: &MockServer, claims: serde_json::Value) -> String {
        let now = chrono::Utc::now().timestamp();
        let mut all = serde_json::json!({
            "iss": server.uri(),
            "sub": "user_01TEST",
            "aud": TEST_CLIENT_ID,
            "exp": now + 600,
            "iat": now,
            "nonce": NONCE,
        });
        if let (Some(all), Some(claims)) = (all.as_object_mut(), claims.as_object()) {
            all.extend(claims.clone());
        }
        provider_id_token(all)
    }

    /// With userinfo read, its claims stand over a token that kept the
    /// address out — read with the grant's access token — and an answer
    /// about another subject is dropped whole.
    #[tokio::test]
    async fn userinfo_fills_what_the_token_kept_out_for_the_same_subject_alone() {
        let server = MockServer::start().await;
        let p = reading_userinfo(&server).await;
        let token = id_token_of(&server, serde_json::json!({ "given_name": "Ada" }));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .and(header("authorization", "Bearer opaque_at_1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sub": "user_01TEST",
                "email": "ada@example.com",
                "email_verified": "true",
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        let authn = p
            .exchange_code("code_1", "https://app.example/cb", PROOF)
            .await
            .unwrap();
        assert_eq!(authn.subject.email.as_deref(), Some("ada@example.com"));
        assert!(authn.subject.email_verified);
        assert_eq!(authn.subject.given_name.as_deref(), Some("Ada"));

        Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "sub": "user_02OTHER",
                "email": "mallory@example.com",
                "email_verified": true,
            })))
            .mount(&server)
            .await;
        let authn = p
            .exchange_code("code_2", "https://app.example/cb", PROOF)
            .await
            .unwrap();
        assert_eq!(
            authn.subject.email, None,
            "another subject's answer is dropped"
        );
    }

    /// A token with `name` alone, as Entra ID's is until its optional claims
    /// are set, names the person by it.
    #[tokio::test]
    async fn the_name_claim_stands_in_where_no_given_or_family_name_came() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        let token = id_token_of(
            &server,
            serde_json::json!({ "email": "ada@example.com", "name": "Ada Lovelace" }),
        );
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .mount(&server)
            .await;
        let authn = p
            .exchange_code("code_1", "https://app.example/cb", PROOF)
            .await
            .unwrap();
        assert_eq!(authn.subject.given_name.as_deref(), Some("Ada Lovelace"));
        assert_eq!(authn.subject.family_name, None);
    }

    /// An address keeps the verdict of whichever answer named it.
    #[test]
    fn an_address_and_its_verdict_come_from_one_answer() {
        let claims =
            |value: serde_json::Value| -> IdTokenClaims { serde_json::from_value(value).unwrap() };
        let pair = |merged: IdTokenClaims| (merged.email, merged.email_verified);
        let token = || {
            claims(
                serde_json::json!({ "sub": "s", "email": "ada@corp.example", "email_verified": true }),
            )
        };

        let other = claims(serde_json::json!({ "sub": "s", "email": "mallory@other.example" }));
        assert_eq!(
            pair(token().merged(other)),
            (Some("mallory@other.example".to_owned()), None),
            "the token's verdict never vouches for userinfo's address"
        );
        let verdict_alone = claims(serde_json::json!({ "sub": "s", "email_verified": true }));
        let unverified = claims(
            serde_json::json!({ "sub": "s", "email": "ada@corp.example", "email_verified": false }),
        );
        assert_eq!(
            pair(unverified.merged(verdict_alone)),
            (Some("ada@corp.example".to_owned()), Some(false)),
            "a verdict with no address leaves the token's pair whole"
        );
        let same = claims(
            serde_json::json!({ "sub": "s", "email": " Ada@Corp.example", "email_verified": "false" }),
        );
        assert_eq!(
            pair(token().merged(same)),
            (Some(" Ada@Corp.example".to_owned()), Some(false)),
            "the same address takes userinfo's verdict"
        );
        let silent = claims(serde_json::json!({ "sub": "s", "email": "ada@corp.example" }));
        assert_eq!(
            pair(token().merged(silent)),
            (Some("ada@corp.example".to_owned()), Some(true)),
            "and the token's where userinfo gave none"
        );
    }

    /// An endpoint that already carries a query string is extended, not
    /// given a second `?`.
    #[test]
    fn a_query_is_appended_to_an_endpoint_that_has_one() {
        assert_eq!(
            with_query("https://idp.example/authorize?tenant=t1", &[("a", "b c")]),
            "https://idp.example/authorize?tenant=t1&a=b%20c"
        );
        assert_eq!(
            with_query("https://idp.example/authorize", &[]),
            "https://idp.example/authorize"
        );
    }

    /// The confidential exchange authenticates with the secret, and the
    /// person comes out of the id token, which is handed back for the
    /// sign-out and nothing else.
    #[tokio::test]
    async fn an_exchange_authenticates_as_the_client_and_names_the_person_from_the_id_token() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        let token = id_token(&server, TEST_CLIENT_ID, Some("session_1"));
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("code=code_xyz"))
            .and(body_string_contains(
                "redirect_uri=http%3A%2F%2Flocalhost%3A3000%2Fauth%2Fcallback",
            ))
            .and(body_string_contains(format!("client_id={TEST_CLIENT_ID}")))
            .and(header("authorization", basic_credentials().as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .expect(1)
            .mount(&server)
            .await;

        let authn = p
            .exchange_code("code_xyz", "http://localhost:3000/auth/callback", PROOF)
            .await
            .expect("the exchange succeeds");
        assert_eq!(authn.subject.sub, "user_01TEST");
        assert_eq!(authn.subject.email.as_deref(), Some("ada@example.com"));
        assert!(authn.subject.email_verified);
        assert_eq!(authn.subject.given_name.as_deref(), Some("Ada"));
        assert_eq!(authn.id_token.as_deref(), Some(token.as_str()));
        assert_eq!(authn.auth_method, None);
    }

    /// ⚠ The id token is verified before anything in it is believed: one
    /// minted for another client is refused even though the provider
    /// answered 200.
    #[tokio::test]
    async fn an_id_token_for_another_client_is_refused() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        let token = id_token(&server, "another_client", Some("session_1"));
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .mount(&server)
            .await;
        let err = p
            .exchange_code("code_xyz", "http://x/cb", PROOF)
            .await
            .unwrap_err();
        assert_eq!(err.to_problem_details().status, 401, "{err:?}");
    }

    /// The session is the issuer's, opened after the exchange, so the
    /// provider's own session id is nothing a sign-in needs: a provider that
    /// names none in its id tokens signs people in like one that does.
    #[tokio::test]
    async fn an_id_token_without_a_provider_session_signs_the_person_in() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        let token = id_token(&server, TEST_CLIENT_ID, None);
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(token_ok(&token)))
            .mount(&server)
            .await;
        let authn = p
            .exchange_code("code_xyz", "http://x/cb", PROOF)
            .await
            .expect("an id token needs no sid");
        assert_eq!(authn.subject.sub, "user_01TEST");
    }

    /// A grant answered without an id token names nobody, and is refused
    /// rather than treated as a person with no claims.
    #[tokio::test]
    async fn a_grant_without_an_id_token_is_refused() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "opaque", "token_type": "Bearer", "expires_in": 300,
            })))
            .mount(&server)
            .await;
        let err = p
            .exchange_code("code_xyz", "http://x/cb", PROOF)
            .await
            .unwrap_err();
        assert_eq!(err.to_problem_details().status, 401, "{err:?}");
    }

    /// A 4xx from the token endpoint is the code's fault and a 401 here; a
    /// 5xx is the provider's and a 500.
    #[tokio::test]
    async fn a_refused_grant_is_a_401_and_an_upstream_failure_a_500() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code=code_dead"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "error": "invalid_grant" })),
            )
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("code=code_500"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&server)
            .await;
        assert_eq!(
            p.exchange_code("code_dead", "http://x/cb", PROOF)
                .await
                .unwrap_err()
                .to_problem_details()
                .status,
            401
        );
        assert_eq!(
            p.exchange_code("code_500", "http://x/cb", PROOF)
                .await
                .unwrap_err()
                .to_problem_details()
                .status,
            500
        );
    }

    /// The browser logout is RP-initiated logout at the published page, and
    /// `return_to` itself where the provider publishes none.
    #[tokio::test]
    async fn logout_url_is_rp_initiated_logout_or_the_return_to_itself() {
        let server = MockServer::start().await;
        let p = provider_at(&server, true).await;
        let url = p.logout_url(Some("id.token.x"), "https://app/x?a=b").await;
        assert_eq!(
            url,
            format!(
                "{}/logout?client_id={TEST_CLIENT_ID}&id_token_hint=id.token.x&post_logout_redirect_uri=https%3A%2F%2Fapp%2Fx%3Fa%3Db",
                server.uri()
            )
        );
        let without_hint = p.logout_url(None, "https://app/x").await;
        assert!(!without_hint.contains("id_token_hint"), "{without_hint}");
        let bare = MockServer::start().await;
        let p = provider_at(&bare, false).await;
        assert_eq!(
            p.logout_url(Some("id.token.x"), "https://app/x").await,
            "https://app/x"
        );
    }

    /// Client authentication follows the provider's document: Basic unless
    /// it offers only `client_secret_post`.
    #[test]
    fn client_auth_is_basic_unless_the_document_offers_only_post() {
        let doc = |methods: Option<&[&str]>| Endpoints {
            issuer: "https://idp.example".into(),
            authorization_endpoint: "https://idp.example/authorize".into(),
            token_endpoint: "https://idp.example/token".into(),
            jwks_uri: "https://idp.example/jwks".into(),
            device_authorization_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
            token_endpoint_auth_methods_supported: methods
                .map(|m| m.iter().map(|s| (*s).to_owned()).collect()),
            userinfo_endpoint: None,
            prompt_values_supported: None,
        };
        assert_eq!(client_auth(&doc(None)), ClientAuth::Basic);
        assert_eq!(
            client_auth(&doc(Some(&["client_secret_basic", "client_secret_post"]))),
            ClientAuth::Basic
        );
        assert_eq!(
            client_auth(&doc(Some(&["client_secret_post", "private_key_jwt"]))),
            ClientAuth::Post
        );
        assert_eq!(
            client_auth(&doc(Some(&["private_key_jwt"]))),
            ClientAuth::Basic
        );
    }

    /// A provider whose document offers only `client_secret_post` gets the
    /// secret in the body and no Authorization header.
    #[tokio::test]
    async fn a_post_only_provider_gets_the_secret_in_the_body() {
        let server = MockServer::start().await;
        let base = server.uri();
        let discovery = Arc::new(Discovery::fixed(Endpoints {
            issuer: base.clone(),
            authorization_endpoint: format!("{base}/authorize"),
            token_endpoint: format!("{base}/token"),
            jwks_uri: format!("{base}/jwks"),
            device_authorization_endpoint: None,
            end_session_endpoint: None,
            revocation_endpoint: None,
            token_endpoint_auth_methods_supported: Some(vec!["client_secret_post".into()]),
            userinfo_endpoint: None,
            prompt_values_supported: None,
        }));
        let p = provider_with(&base, discovery, None);
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("client_secret=secret_123"))
            .respond_with(
                ResponseTemplate::new(400)
                    .set_body_json(serde_json::json!({ "error": "invalid_grant" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            p.exchange_code("code_1", "http://x/cb", PROOF)
                .await
                .is_err()
        );
        let sent = &server.received_requests().await.unwrap()[0];
        assert!(sent.headers.get("authorization").is_none());
    }

    /// The account-management calls are the trait's defaults: nothing is
    /// deleted or minted, and a caller is told whose job it is.
    #[tokio::test]
    async fn account_management_is_the_providers_own_affair() {
        let server = MockServer::start().await;
        let p = provider_at(&server, false).await;
        p.delete_user("user_1")
            .await
            .expect("nothing to do is done");
        for err in [
            p.create_password_reset("ada@example.com")
                .await
                .unwrap_err(),
            p.send_email_change("user_1", "new@example.com")
                .await
                .unwrap_err(),
            p.confirm_email_change("user_1", "123456")
                .await
                .unwrap_err(),
        ] {
            assert_eq!(err.to_problem_details().status, 403, "{err:?}");
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
