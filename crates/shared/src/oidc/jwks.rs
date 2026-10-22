//! An identity provider's key set, cached in the process and refreshed on a
//! miss.
//!
//! Held closed: with nothing cached and the provider unreachable, an id token
//! cannot be checked and is not taken — the same posture as an unreadable
//! flag set or a KEK that will not answer.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::error::{AuthError, TelmoniError};

use super::id_token::SigningAlg;

/// How long a fetched set is trusted before a routine refresh.
const TTL: Duration = Duration::from_secs(600);

/// The least time between two fetches, so a stream of unknown `kid`s (a
/// forger's, say) cannot turn this service into a request generator against
/// the provider.
const RETRY_EVERY: Duration = Duration::from_secs(60);

/// One public key as the set publishes it, in the shape `ring` verifies
/// against with no DER in between: an RSA key's big-endian `n` and `e`, or
/// an EC key's uncompressed point on its curve; and the one algorithm the
/// set says it is for, where it says one.
pub struct PublicKey {
    material: Material,
    alg: Option<String>,
}

enum Material {
    Rsa { n: Vec<u8>, e: Vec<u8> },
    Ec { curve: Curve, point: Vec<u8> },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Curve {
    P256,
    P384,
}

impl PublicKey {
    /// Whether `signature` over `message` verifies under this key by `alg`:
    /// never where the key is of another kind or curve, or where the set
    /// published it for another algorithm.
    #[must_use]
    pub fn verifies(&self, alg: SigningAlg, message: &[u8], signature: &[u8]) -> bool {
        use ring::signature as sig;
        if self
            .alg
            .as_deref()
            .is_some_and(|published| published != alg.as_str())
        {
            return false;
        }
        match (&self.material, alg) {
            (Material::Rsa { n, e }, rsa) => {
                let algorithm: &'static sig::RsaParameters = match rsa {
                    SigningAlg::Rs256 => &sig::RSA_PKCS1_2048_8192_SHA256,
                    SigningAlg::Rs384 => &sig::RSA_PKCS1_2048_8192_SHA384,
                    SigningAlg::Rs512 => &sig::RSA_PKCS1_2048_8192_SHA512,
                    SigningAlg::Ps256 => &sig::RSA_PSS_2048_8192_SHA256,
                    SigningAlg::Ps384 => &sig::RSA_PSS_2048_8192_SHA384,
                    SigningAlg::Ps512 => &sig::RSA_PSS_2048_8192_SHA512,
                    SigningAlg::Es256 | SigningAlg::Es384 => return false,
                };
                sig::RsaPublicKeyComponents {
                    n: &n[..],
                    e: &e[..],
                }
                .verify(algorithm, message, signature)
                .is_ok()
            }
            (Material::Ec { curve, point }, ec) => {
                // A JWS signature is the fixed-width `r || s`, not ASN.1.
                let algorithm: &'static sig::EcdsaVerificationAlgorithm = match (curve, ec) {
                    (Curve::P256, SigningAlg::Es256) => &sig::ECDSA_P256_SHA256_FIXED,
                    (Curve::P384, SigningAlg::Es384) => &sig::ECDSA_P384_SHA384_FIXED,
                    _ => return false,
                };
                sig::UnparsedPublicKey::new(algorithm, point)
                    .verify(message, signature)
                    .is_ok()
            }
        }
    }
}

#[derive(Deserialize)]
struct Jwk {
    kid: Option<String>,
    kty: String,
    #[serde(rename = "use")]
    use_: Option<String>,
    alg: Option<String>,
    n: Option<String>,
    e: Option<String>,
    crv: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

#[derive(Deserialize)]
struct Jwks {
    keys: Vec<Jwk>,
}

/// One base64url member of a key, or why it is not.
fn member(kid: &str, name: &str, value: Option<String>) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(value.ok_or_else(|| format!("key {kid} has no {name}"))?)
        .map_err(|e| format!("key {kid}: {name} is not base64url: {e}"))
}

/// Parse a JWK set into the keys a signature can be checked against, by
/// `kid`: RSA keys, and EC keys on P-256 or P-384, the curves `ring`
/// verifies. Keys of another type or curve are skipped, and so is a key the
/// set says is for something other than the signatures verified here (`use`,
/// `alg`): a key published for encryption is never one a signature is
/// checked against. A malformed
/// key of a kind taken is an error, because a set that half-parses would
/// refuse real tokens for a reason nobody can see.
pub fn parse(json: &str) -> Result<HashMap<String, Arc<PublicKey>>, String> {
    let set: Jwks = serde_json::from_str(json).map_err(|e| format!("key set is not JSON: {e}"))?;
    let mut keys = HashMap::new();
    for jwk in set.keys {
        if jwk.use_.as_deref().is_some_and(|u| u != "sig")
            || jwk
                .alg
                .as_deref()
                .is_some_and(|alg| alg.parse::<SigningAlg>().is_err())
        {
            continue;
        }
        let curve = match (jwk.kty.as_str(), jwk.crv.as_deref()) {
            ("RSA", _) => None,
            ("EC", Some("P-256")) => Some(Curve::P256),
            ("EC", Some("P-384")) => Some(Curve::P384),
            _ => continue,
        };
        let kid = jwk
            .kid
            .filter(|k| !k.is_empty())
            .ok_or("a signing key with no kid")?;
        let material = match curve {
            None => Material::Rsa {
                n: member(&kid, "n", jwk.n)?,
                e: member(&kid, "e", jwk.e)?,
            },
            Some(curve) => {
                let width = if curve == Curve::P256 { 32 } else { 48 };
                let (x, y) = (member(&kid, "x", jwk.x)?, member(&kid, "y", jwk.y)?);
                if x.len() != width || y.len() != width {
                    return Err(format!("key {kid}: x and y are not its curve's width"));
                }
                // The uncompressed point: 0x04, then x, then y.
                let mut point = Vec::with_capacity(1 + 2 * width);
                point.push(0x04);
                point.extend_from_slice(&x);
                point.extend_from_slice(&y);
                Material::Ec { curve, point }
            }
        };
        keys.insert(
            kid,
            Arc::new(PublicKey {
                material,
                alg: jwk.alg,
            }),
        );
    }
    Ok(keys)
}

/// Where the key set is: a URL known at boot, or the `jwks_uri` of a
/// provider's discovery document, read when the first fetch needs it.
pub enum KeySetUrl {
    /// The full key-set URL.
    Fixed(String),
    /// The `jwks_uri` the issuer's discovery document names.
    Discovered(Arc<super::Discovery>),
}

impl std::fmt::Display for KeySetUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed(url) => f.write_str(url),
            Self::Discovered(discovery) => write!(f, "{} (jwks_uri)", discovery.document_url()),
        }
    }
}

struct Inner {
    keys: HashMap<String, Arc<PublicKey>>,
    fetched_at: Option<Instant>,
    attempted_at: Option<Instant>,
}

/// The provider's keys.
///
/// ⚠ **No lock is held across the network.** One task fetches at a time
/// (`refreshing`); `inner` is taken only to read the set or swap in a new
/// one. While a fetch is in flight, a request whose key is cached — stale
/// or not — is answered from the cache, and only a request with nothing to
/// verify against waits for the fetch. Holding the set's write lock across
/// the fetch once stalled every verification on the instance for as long as
/// the provider took to answer, once per refresh.
pub struct JwksCache {
    http: reqwest::Client,
    url: KeySetUrl,
    inner: RwLock<Inner>,
    refreshing: Mutex<()>,
}

impl JwksCache {
    /// The key set at `url`, fetched when the first token needs it.
    #[must_use]
    pub fn new(http: reqwest::Client, url: KeySetUrl) -> Self {
        Self {
            http,
            url,
            inner: RwLock::new(Inner {
                keys: HashMap::new(),
                fetched_at: None,
                attempted_at: None,
            }),
            refreshing: Mutex::new(()),
        }
    }

    /// The key for `kid`, fetching the set when it is unknown or stale.
    /// `Ok(None)` is an unknown kid the provider does not publish either;
    /// `Err` is a set that could not be read with nothing cached to fall
    /// back on.
    pub async fn key_for(&self, kid: &str) -> Result<Option<Arc<PublicKey>>, TelmoniError> {
        let cached = {
            let inner = self.inner.read().await;
            let key = inner.keys.get(kid).cloned();
            if key.is_some() && inner.fetched_at.is_some_and(|at| at.elapsed() < TTL) {
                return Ok(key);
            }
            key
        };
        // Stale, or a kid the cached set lacks. One task refreshes; a request
        // holding a stale key is answered with it rather than queued behind
        // the fetch, and one holding nothing waits for that fetch's answer.
        let _refreshing = match (self.refreshing.try_lock(), cached) {
            (Ok(guard), _) => guard,
            (Err(_), Some(stale)) => return Ok(Some(stale)),
            (Err(_), None) => self.refreshing.lock().await,
        };
        if let Err(e) = self.refresh_if_due().await {
            if self.inner.read().await.keys.is_empty() {
                tracing::error!(error = %e, url = %self.url, "provider key set unreadable and nothing cached");
                return Err(unavailable());
            }
            tracing::warn!(error = %e, url = %self.url, "provider key set refresh failed; serving the cached set");
        }
        let inner = self.inner.read().await;
        if inner.keys.is_empty() {
            return Err(unavailable());
        }
        Ok(inner.keys.get(kid).cloned())
    }

    /// Fetch unless one was attempted within [`RETRY_EVERY`] — by this task's
    /// predecessor in the queue, most often, whose answer is the answer. The
    /// caller holds `refreshing`.
    async fn refresh_if_due(&self) -> Result<(), String> {
        {
            let mut inner = self.inner.write().await;
            if inner
                .attempted_at
                .is_some_and(|at| at.elapsed() < RETRY_EVERY)
            {
                return Ok(());
            }
            inner.attempted_at = Some(Instant::now());
        }
        self.fetch_and_swap().await
    }

    /// The fetch, with no lock on the set; then the swap, under it briefly.
    async fn fetch_and_swap(&self) -> Result<(), String> {
        let keys = self.fetch().await?;
        let mut inner = self.inner.write().await;
        inner.keys = keys;
        inner.fetched_at = Some(Instant::now());
        Ok(())
    }

    async fn fetch(&self) -> Result<HashMap<String, Arc<PublicKey>>, String> {
        let url = match &self.url {
            KeySetUrl::Fixed(url) => url.clone(),
            KeySetUrl::Discovered(discovery) => discovery
                .endpoints()
                .await
                .map(|e| e.jwks_uri.clone())
                .map_err(|_| format!("discovery at {} unreadable", discovery.document_url()))?,
        };
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("unreachable: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("answered {}", resp.status()));
        }
        let body = resp
            .text()
            .await
            .map_err(|e| format!("body unreadable: {e}"))?;
        parse(&body)
    }
}

fn unavailable() -> TelmoniError {
    AuthError::IdentityUnavailable.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys of a kind or curve `ring` cannot verify are skipped; a broken key
    /// of a kind taken is an error.
    #[test]
    fn unverifiable_keys_are_skipped_and_a_broken_one_is_an_error() {
        let skipped = r#"{"keys":[
            {"kty":"EC","kid":"ec5","crv":"P-521","x":"a","y":"b"},
            {"kty":"oct","kid":"o1","k":"c2VjcmV0"},
            {"kty":"RSA","kid":"enc","use":"enc","n":"AQAB","e":"AQAB"}
        ]}"#;
        assert!(parse(skipped).unwrap().is_empty());
        let broken = r#"{"keys":[{"kty":"RSA","kid":"r1","n":"not base64url!!","e":"AQAB"}]}"#;
        assert!(parse(broken).is_err());
        let short = r#"{"keys":[{"kty":"EC","kid":"ec1","crv":"P-256","x":"AQAB","y":"AQAB"}]}"#;
        assert!(parse(short).is_err(), "a point of the wrong width");
    }

    /// ⚠ A refresh in flight must not queue the requests that already hold a
    /// key: the provider answers slowly here, and the second verification is
    /// served the stale key at once instead of waiting it out.
    #[tokio::test]
    async fn a_stale_key_is_served_while_a_slow_refresh_is_in_flight() {
        use crate::test_util::id_token::{PROVIDER_KID, provider_jwks_json};

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(provider_jwks_json())
                    .set_delay(Duration::from_secs(2)),
            )
            .expect(1)
            .mount(&server)
            .await;
        let cache = Arc::new(JwksCache::new(
            reqwest::Client::new(),
            KeySetUrl::Fixed(server.uri()),
        ));
        {
            let mut inner = cache.inner.write().await;
            inner.keys = parse(&provider_jwks_json()).unwrap();
            let long_ago = Instant::now()
                .checked_sub(TTL + RETRY_EVERY)
                .expect("the clock has run longer than one TTL");
            inner.fetched_at = Some(long_ago);
            inner.attempted_at = Some(long_ago);
        }

        let refresher = {
            let cache = Arc::clone(&cache);
            tokio::spawn(async move { cache.key_for(PROVIDER_KID).await })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;

        let started = Instant::now();
        assert!(cache.key_for(PROVIDER_KID).await.unwrap().is_some());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a request holding a stale key waited {:?} for somebody else's refresh",
            started.elapsed()
        );
        assert!(refresher.await.unwrap().unwrap().is_some());
        drop(server);
    }

    /// A key for encryption is never a signing key, and one published for an
    /// algorithm verifies by that algorithm alone.
    #[test]
    fn a_key_for_encryption_is_never_a_signing_key_nor_one_for_another_algorithm() {
        let key = |extra: &str| {
            format!(r#"{{"keys":[{{"kty":"RSA","kid":"r1","n":"AQAB","e":"AQAB"{extra}}}]}}"#)
        };
        assert!(parse(&key("")).unwrap().contains_key("r1"));
        assert!(
            parse(&key(r#","use":"sig","alg":"RS256""#))
                .unwrap()
                .contains_key("r1")
        );
        assert!(parse(&key(r#","use":"enc""#)).unwrap().is_empty());
        assert!(parse(&key(r#","alg":"RSA-OAEP""#)).unwrap().is_empty());
        let rs512 = parse(&key(r#","alg":"RS512""#)).unwrap();
        assert!(!rs512["r1"].verifies(SigningAlg::Rs256, b"m", b"s"));
    }
}
