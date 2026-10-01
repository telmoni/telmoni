//! The key-encryption key: what wraps each row's data key.
//!
//! On a deployed tier the KEK is a Cloud KMS key the process never holds: each
//! open is a `:decrypt` call and a line in Cloud Audit Logs naming the caller.
//! On a laptop it is a local AES-256-GCM key, refused at boot in a pod.
//!
//! **Two answers, and only two.** [`KekError::Invalid`] means this row is
//! broken and no retry will change it. Everything else — network, 5xx, a
//! refused credential, a disabled key version — is [`KekError::Unavailable`],
//! and a queue holds the row rather than spending an attempt. A disabled key
//! version is deliberately in the second bucket: that is the kill switch
//! working, and every row must survive it being turned back on.
//!
//! ⚠ **The mapping reads the JSON `error.status`, never the HTTP status
//! alone.** An identity-provider adapter once matched a bare 404 on a path the
//! provider did not serve and reported success for months. The same shortcut
//! here would read a misrouted `KMS_API_BASE` as a broken row. Unknown statuses
//! hold after a `warn!`, so a wrong guess holds a row rather than losing it.
//!
//! No DEK is cached: every open is on an async queue where a round trip is
//! invisible, and a process holding no long-lived key material is the point.

use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use zeroize::Zeroizing;

use super::{DEK_LEN, Dek, NONCE_LEN};
use crate::Redacted;

/// The KEK generation every row wrapped by this build carries. Moves only
/// when the KEK becomes a different key RESOURCE; a KMS version rotation needs
/// nothing here, because KMS ciphertext names its own version.
pub const KEK_VERSION: i16 = 1;

/// The spelling of a laptop's KEK: `local:` and 64 hex characters.
const LOCAL_PREFIX: &str = "local:";

/// A metadata-server token is dropped this long before it expires, so a
/// call never starts on a bearer that dies in flight.
const TOKEN_REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// The most of a KMS error message that reaches a log line or a row.
const MESSAGE_MAX: usize = 200;

/// Why a DEK did not come back: *hold*, or *this row is broken*.
#[derive(Debug)]
pub enum KekError {
    /// The KEK cannot be reached or will not answer right now. Nothing about
    /// the row; hold it and try again later.
    Unavailable(String),
    /// The wrapped key will not open under this row — corrupt bytes, a
    /// foreign key version, a key moved between rows. Retrying changes nothing.
    Invalid(String),
}

impl std::fmt::Display for KekError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => write!(f, "kek unavailable: {why}"),
            Self::Invalid(why) => write!(f, "wrapped key invalid: {why}"),
        }
    }
}

impl std::error::Error for KekError {}

/// The configured KEK.
pub enum Kek {
    /// A Cloud KMS key, named in full. The deployed shape.
    Kms(KmsKek),
    /// An AES-256-GCM key from the environment. A laptop's shape only. Boxed:
    /// the key schedule is half a kilobyte the KMS arm does not carry.
    Local(Box<LocalKek>),
}

impl std::fmt::Debug for Kek {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kms(kms) => f.debug_struct("Kek::Kms").field("name", &kms.name).finish(),
            Self::Local(_) => f.write_str("Kek::Local"),
        }
    }
}

impl Kek {
    /// Build from an environment value: a Cloud KMS key name or `local:<64 hex>`.
    /// A malformed value is named at boot, because a service with a bad KEK
    /// would refuse every write and hold every read.
    pub fn parse(
        var: &str,
        value: &Redacted,
        kms_api_base: &str,
        metadata_api_base: &str,
    ) -> Result<Self, String> {
        let value = value.expose().trim();
        if let Some(hex_key) = value.strip_prefix(LOCAL_PREFIX) {
            return LocalKek::from_hex(hex_key)
                .map(|k| Self::Local(Box::new(k)))
                .map_err(|e| format!("{var}: {e}"));
        }
        KmsKek::new(value, kms_api_base, metadata_api_base)
            .map(Self::Kms)
            .map_err(|e| format!("{var}: {e}"))
    }

    /// `"kms"` or `"local"`, for the boot log.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Kms(_) => "kms",
            Self::Local(_) => "local",
        }
    }

    /// Whether this is a laptop's key, which a deployed tier must refuse.
    #[must_use]
    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }

    /// Wrap a fresh DEK for one row. `aad` is the row's id, so a wrapped key
    /// copied to another row will not open there. A write treats either error
    /// as a refusal: nothing is stored under a key that did not wrap.
    pub async fn wrap(
        &self,
        http: &reqwest::Client,
        dek: &Dek,
        aad: &[u8],
    ) -> Result<Vec<u8>, KekError> {
        match self {
            Self::Kms(kms) => kms.wrap(http, dek, aad).await,
            Self::Local(local) => local.wrap(dek, aad),
        }
    }

    /// Open a row's wrapped DEK. A `key_version` this build does not hold is
    /// refused before any call, rather than spending a round trip to learn it.
    pub async fn unwrap_dek(
        &self,
        http: &reqwest::Client,
        wrapped: &[u8],
        aad: &[u8],
        key_version: i16,
    ) -> Result<Dek, KekError> {
        if key_version != KEK_VERSION {
            return Err(KekError::Invalid(format!(
                "row is wrapped under key version {key_version}; this build holds {KEK_VERSION}"
            )));
        }
        match self {
            Self::Kms(kms) => kms.unwrap_dek(http, wrapped, aad).await,
            Self::Local(local) => local.unwrap_dek(wrapped, aad),
        }
    }
}

/// Whether this process is running on a deployed tier — the signal a service
/// reads at boot to refuse what only a laptop may use: a `local:` KEK, a test
/// issuer's key set, an identity provider that is not HTTPS, a database with
/// no pinned CA.
///
/// Kubernetes sets `KUBERNETES_SERVICE_HOST` in every container of every pod —
/// with service links turned off too, which every workload here does — and a
/// laptop sets it nowhere. Nothing has to remember to set it, which is the
/// point: a signal a manifest could omit would pass all four refusals as
/// silently as a laptop. Not "is `APP_URL` loopback": that refused a laptop
/// behind a tunnel — the shape Slack's HTTPS-only URLs require.
#[must_use]
pub fn on_deployed_tier() -> bool {
    deployed_from(std::env::var_os("KUBERNETES_SERVICE_HOST").as_deref())
}

/// The decision behind [`on_deployed_tier`], pure so it can be pinned.
fn deployed_from(kubernetes_service_host: Option<&std::ffi::OsStr>) -> bool {
    kubernetes_service_host.is_some_and(|host| !host.is_empty())
}

/// Whether `origin` names this machine: the three spellings of loopback.
#[must_use]
pub fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// A bearer from the metadata server, and when to stop trusting it.
struct CachedToken {
    bearer: Redacted,
    good_until: Instant,
}

/// A Cloud KMS key, spoken to over REST with a token from the metadata server.
/// No SDK: a hand-rolled REST call beats pulling a gRPC dependency graph
/// through `cargo deny`.
pub struct KmsKek {
    /// `projects/…/locations/…/keyRings/…/cryptoKeys/…`. The key, never a
    /// version: KMS picks the version each ciphertext names on decrypt, and
    /// uses the primary on encrypt.
    name: String,
    api_base: String,
    metadata_base: String,
    /// Fetched once and reused until shortly before it expires. A `Redacted`,
    /// so it reaches no error string.
    token: tokio::sync::Mutex<Option<CachedToken>>,
}

impl KmsKek {
    /// A KMS KEK by its full key resource name, with the API and metadata
    /// origins (the tests point both at a mock).
    pub fn new(name: &str, api_base: &str, metadata_base: &str) -> Result<Self, String> {
        if name.contains("/cryptoKeyVersions/") {
            return Err(
                "the value names a key VERSION; name the key itself — KMS picks the version \
                 from each ciphertext, and a pinned version cannot open a row wrapped under the \
                 one before it"
                    .to_owned(),
            );
        }
        let segment_ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        let parts: Vec<&str> = name.split('/').collect();
        match parts.as_slice() {
            [
                "projects",
                project,
                "locations",
                location,
                "keyRings",
                ring,
                "cryptoKeys",
                key,
            ] if [project, location, ring, key]
                .into_iter()
                .all(|segment| segment_ok(segment)) => {}
            _ => {
                return Err(
                    "the value is neither `local:<64 hex>` nor a Cloud KMS key name \
                     (projects/<p>/locations/<l>/keyRings/<r>/cryptoKeys/<k>)"
                        .to_owned(),
                );
            }
        }
        Ok(Self {
            name: name.to_owned(),
            api_base: api_base.trim_end_matches('/').to_owned(),
            metadata_base: metadata_base.trim_end_matches('/').to_owned(),
            token: tokio::sync::Mutex::new(None),
        })
    }

    async fn wrap(
        &self,
        http: &reqwest::Client,
        dek: &Dek,
        aad: &[u8],
    ) -> Result<Vec<u8>, KekError> {
        #[derive(Deserialize)]
        struct Encrypted {
            ciphertext: String,
        }
        let body = json!({
            "plaintext": STANDARD.encode(dek.as_bytes()),
            "additionalAuthenticatedData": STANDARD.encode(aad),
        });
        let answer: Encrypted = self.call(http, "encrypt", &body).await?;
        STANDARD.decode(answer.ciphertext).map_err(|e| {
            KekError::Unavailable(format!(
                "kms encrypt answered with ciphertext that is not base64: {e}"
            ))
        })
    }

    async fn unwrap_dek(
        &self,
        http: &reqwest::Client,
        wrapped: &[u8],
        aad: &[u8],
    ) -> Result<Dek, KekError> {
        #[derive(Deserialize)]
        struct Decrypted {
            plaintext: Redacted,
        }
        let body = json!({
            "ciphertext": STANDARD.encode(wrapped),
            "additionalAuthenticatedData": STANDARD.encode(aad),
        });
        let answer: Decrypted = self.call(http, "decrypt", &body).await?;
        let bytes = Zeroizing::new(STANDARD.decode(answer.plaintext.expose()).map_err(|e| {
            KekError::Unavailable(format!(
                "kms decrypt answered with plaintext that is not base64: {e}"
            ))
        })?);
        let dek: [u8; DEK_LEN] = bytes.as_slice().try_into().map_err(|_| {
            KekError::Invalid(format!(
                "kms decrypt opened the wrapped key to {} bytes; a data key is {DEK_LEN}",
                bytes.len()
            ))
        })?;
        Ok(Dek::from_bytes(dek))
    }

    /// One `:encrypt` or `:decrypt`, classified by [`classify`].
    async fn call<T: DeserializeOwned>(
        &self,
        http: &reqwest::Client,
        op: &str,
        body: &serde_json::Value,
    ) -> Result<T, KekError> {
        let bearer = self.bearer(http).await?;
        let resp = http
            .post(format!("{}/v1/{}:{op}", self.api_base, self.name))
            .bearer_auth(bearer.expose())
            .json(body)
            .send()
            .await
            .map_err(|e| KekError::Unavailable(format!("kms {op} unreachable: {e}")))?;
        let status = resp.status();
        if status.is_success() {
            return resp.json().await.map_err(|e| {
                KekError::Unavailable(format!(
                    "kms {op} answered {status} with a body that is not the documented one: {e}"
                ))
            });
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            // The bearer we hold was refused; fetch a fresh one next time.
            self.token.lock().await.take();
        }
        let text = resp.text().await.unwrap_or_default();
        Err(classify(op, status, &text))
    }

    /// The cached bearer, or a fresh one from the metadata server. The lock
    /// is held across the fetch on purpose: twenty rows opening at once in a
    /// new pod should fetch one token, not twenty.
    async fn bearer(&self, http: &reqwest::Client) -> Result<Redacted, KekError> {
        let mut slot = self.token.lock().await;
        if let Some(cached) = slot.as_ref()
            && Instant::now() < cached.good_until
        {
            return Ok(cached.bearer.clone());
        }
        let fresh = self.fetch_token(http).await?;
        let bearer = fresh.bearer.clone();
        *slot = Some(fresh);
        Ok(bearer)
    }

    async fn fetch_token(&self, http: &reqwest::Client) -> Result<CachedToken, KekError> {
        #[derive(Deserialize)]
        struct Token {
            access_token: Redacted,
            expires_in: u64,
        }
        let resp = http
            .get(format!(
                "{}/computeMetadata/v1/instance/service-accounts/default/token",
                self.metadata_base
            ))
            .header("Metadata-Flavor", "Google")
            .send()
            .await
            .map_err(|e| KekError::Unavailable(format!("metadata server unreachable: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(KekError::Unavailable(format!(
                "metadata server answered {status} to the token request"
            )));
        }
        let token: Token = resp
            .json()
            .await
            .map_err(|e| KekError::Unavailable(format!("metadata server token unreadable: {e}")))?;
        let ttl = Duration::from_secs(token.expires_in).saturating_sub(TOKEN_REFRESH_MARGIN);
        Ok(CachedToken {
            bearer: token.access_token,
            good_until: Instant::now() + ttl,
        })
    }
}

/// A non-2xx KMS answer as one of the two outcomes.
///
/// `INVALID_ARGUMENT` is the one status that names the ROW (the ciphertext or
/// AAD does not verify). Everything else names us, the key or KMS, and holds.
/// `FAILED_PRECONDITION` for a disabled key version is assumed from the API
/// docs, not yet observed live; it holds either way. Stamp a date once seen.
fn classify(op: &str, status: reqwest::StatusCode, body: &str) -> KekError {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<Detail>,
    }
    #[derive(Deserialize)]
    struct Detail {
        #[serde(default)]
        status: String,
        #[serde(default)]
        message: String,
    }
    let detail = serde_json::from_str::<Envelope>(body)
        .ok()
        .and_then(|e| e.error);
    let code = detail.as_ref().map_or("", |d| d.status.as_str());
    let message = detail.as_ref().map_or("", |d| {
        crate::text::truncate_on_char_boundary(&d.message, MESSAGE_MAX)
    });
    match code {
        "INVALID_ARGUMENT" => KekError::Invalid(format!("kms {op}: {code}: {message}")),
        "FAILED_PRECONDITION"
        | "PERMISSION_DENIED"
        | "UNAUTHENTICATED"
        | "NOT_FOUND"
        | "RESOURCE_EXHAUSTED"
        | "UNAVAILABLE"
        | "DEADLINE_EXCEEDED"
        | "INTERNAL" => KekError::Unavailable(format!("kms {op}: {code}: {message}")),
        "" => {
            tracing::warn!(
                op,
                status = status.as_u16(),
                "kms answered with no error status in the body; holding the row"
            );
            KekError::Unavailable(format!("kms {op} answered {status} with no error status"))
        }
        unknown => {
            tracing::warn!(
                op,
                status = status.as_u16(),
                code = unknown,
                "kms answered with an error status this build does not know; holding the row"
            );
            KekError::Unavailable(format!("kms {op}: {unknown}: {message}"))
        }
    }
}

/// AES-256-GCM key wrap from a key in the environment. A real KEK with the
/// same contract as KMS (a wrong row id fails), not a fake of it. The wrapped
/// shape is `nonce ‖ ciphertext ‖ tag`.
pub struct LocalKek {
    key: LessSafeKey,
    rng: SystemRandom,
}

impl LocalKek {
    /// From 64 hex characters, which is what `openssl rand -hex 32` prints.
    pub fn from_hex(hex_key: &str) -> Result<Self, String> {
        let bytes = decode_hex(hex_key.trim()).map_err(|e| format!("local key is not hex: {e}"))?;
        if bytes.len() != DEK_LEN {
            return Err(format!(
                "local key is {} bytes; AES-256 needs {DEK_LEN}",
                bytes.len()
            ));
        }
        let unbound = UnboundKey::new(&AES_256_GCM, &bytes)
            .map_err(|_| "local key was refused by the cipher".to_owned())?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
            rng: SystemRandom::new(),
        })
    }

    fn wrap(&self, dek: &Dek, aad: &[u8]) -> Result<Vec<u8>, KekError> {
        let mut nonce = [0u8; NONCE_LEN];
        self.rng
            .fill(&mut nonce)
            .map_err(|_| KekError::Unavailable("no randomness for a wrap nonce".into()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + DEK_LEN + AES_256_GCM.tag_len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(dek.as_bytes());
        // Sealed in place, so the plaintext copy in `out` is overwritten by its
        // own ciphertext rather than left behind.
        let in_out = out
            .get_mut(NONCE_LEN..)
            .ok_or_else(|| KekError::Unavailable("wrap buffer shorter than its nonce".into()))?;
        let tag = self
            .key
            .seal_in_place_separate_tag(Nonce::assume_unique_for_key(nonce), Aad::from(aad), in_out)
            .map_err(|_| KekError::Unavailable("the cipher refused to wrap".into()))?;
        out.extend_from_slice(tag.as_ref());
        Ok(out)
    }

    fn unwrap_dek(&self, wrapped: &[u8], aad: &[u8]) -> Result<Dek, KekError> {
        let Some((nonce, sealed)) = wrapped.split_at_checked(NONCE_LEN) else {
            return Err(KekError::Invalid(
                "wrapped key is shorter than its nonce".into(),
            ));
        };
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| KekError::Invalid("wrapped key nonce is not 96 bits".into()))?;
        let mut in_out = Zeroizing::new(sealed.to_vec());
        let opened = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(aad),
                in_out.as_mut_slice(),
            )
            .map_err(|_| {
                KekError::Invalid("wrapped key did not verify under this row's id".into())
            })?;
        let dek: [u8; DEK_LEN] = (&*opened).try_into().map_err(|_| {
            KekError::Invalid(format!(
                "wrapped key opened to {} bytes; a data key is {DEK_LEN}",
                opened.len()
            ))
        })?;
        Ok(Dek::from_bytes(dek))
    }
}

/// Hex to bytes, wiped on drop. Hand-rolled: this crate has no other hex
/// reader, and the spelling is a laptop's only.
fn decode_hex(hex: &str) -> Result<Zeroizing<Vec<u8>>, String> {
    let digits = hex.as_bytes();
    if !digits.len().is_multiple_of(2) {
        return Err("odd number of digits".to_owned());
    }
    let nibble = |c: u8| match c {
        b'0'..=b'9' => Ok(c - b'0'),
        b'a'..=b'f' => Ok(c - b'a' + 10),
        b'A'..=b'F' => Ok(c - b'A' + 10),
        _ => Err(format!("`{}` is not a hex digit", char::from(c))),
    };
    let mut out = Zeroizing::new(Vec::with_capacity(digits.len() / 2));
    for pair in digits.chunks(2) {
        let &[hi, lo] = pair else {
            return Err("odd number of digits".to_owned());
        };
        out.push((nibble(hi)? << 4) | nibble(lo)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Vault;

    const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    const API: &str = "https://cloudkms.googleapis.com";
    const METADATA: &str = "http://metadata.google.internal";

    fn parse(value: &str) -> Result<Kek, String> {
        Kek::parse("CONNECTOR_KEK", &Redacted::from(value), API, METADATA)
    }

    fn local() -> Kek {
        parse(&format!("local:{KEY}")).unwrap()
    }

    fn http() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test]
    async fn a_wrapped_key_opens_back_to_itself_under_its_row() {
        let kek = local();
        let dek = Vault::new().new_dek().unwrap();
        let wrapped = kek.wrap(&http(), &dek, b"row-1").await.unwrap();
        assert_eq!(wrapped.len(), NONCE_LEN + DEK_LEN + 16);
        assert!(
            !wrapped.windows(DEK_LEN).any(|w| w == dek.as_bytes()),
            "the wrapped key carries the data key in the clear"
        );
        let opened = kek
            .unwrap_dek(&http(), &wrapped, b"row-1", KEK_VERSION)
            .await
            .unwrap();
        assert_eq!(opened.as_bytes(), dek.as_bytes());
    }

    /// The property the AAD buys: a wrapped key moved between rows fails.
    #[tokio::test]
    async fn a_wrapped_key_moved_to_another_row_is_invalid() {
        let kek = local();
        let dek = Vault::new().new_dek().unwrap();
        let wrapped = kek.wrap(&http(), &dek, b"row-1").await.unwrap();
        let err = kek
            .unwrap_dek(&http(), &wrapped, b"row-2", KEK_VERSION)
            .await
            .unwrap_err();
        assert!(matches!(err, KekError::Invalid(_)), "{err}");
    }

    #[tokio::test]
    async fn corrupt_bytes_are_invalid_not_unavailable() {
        let kek = local();
        let dek = Vault::new().new_dek().unwrap();
        let mut wrapped = kek.wrap(&http(), &dek, b"row").await.unwrap();
        if let Some(last) = wrapped.last_mut() {
            *last ^= 0x01;
        }
        let err = kek
            .unwrap_dek(&http(), &wrapped, b"row", KEK_VERSION)
            .await
            .unwrap_err();
        assert!(matches!(err, KekError::Invalid(_)), "{err}");
        let err = kek
            .unwrap_dek(&http(), b"short", b"row", KEK_VERSION)
            .await
            .unwrap_err();
        assert!(matches!(err, KekError::Invalid(_)), "{err}");
    }

    #[tokio::test]
    async fn two_wraps_of_one_key_never_share_a_nonce_or_a_ciphertext() {
        let kek = local();
        let dek = Vault::new().new_dek().unwrap();
        let a = kek.wrap(&http(), &dek, b"row").await.unwrap();
        let b = kek.wrap(&http(), &dek, b"row").await.unwrap();
        assert_ne!(a, b);
        assert_ne!(a.get(..NONCE_LEN), b.get(..NONCE_LEN));
    }

    /// A foreign generation is refused before any call: against a KMS key
    /// this would otherwise spend a round trip to learn what the column says.
    #[tokio::test]
    async fn a_row_under_another_key_version_is_refused_not_tried() {
        let kek = local();
        let dek = Vault::new().new_dek().unwrap();
        let wrapped = kek.wrap(&http(), &dek, b"row").await.unwrap();
        let err = kek
            .unwrap_dek(&http(), &wrapped, b"row", KEK_VERSION + 1)
            .await
            .unwrap_err();
        assert!(matches!(err, KekError::Invalid(_)), "{err}");
    }

    #[test]
    fn a_malformed_kek_is_named_at_load() {
        let err = parse("local:not-hex").unwrap_err();
        assert!(
            err.starts_with("CONNECTOR_KEK: ") && err.contains("not hex"),
            "{err}"
        );
        assert!(parse("local:0011").unwrap_err().contains("needs 32"));
        assert!(
            parse(KEY).unwrap_err().contains("neither"),
            "a bare hex key is not a spelling; it must say local:"
        );
        assert!(
            parse("projects/p/locations/l/keyRings/r")
                .unwrap_err()
                .contains("neither")
        );
        assert!(
            parse("projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/3")
                .unwrap_err()
                .contains("VERSION")
        );
    }

    #[test]
    fn a_kms_name_parses_to_the_kms_kek() {
        let kek = parse(
            "projects/telmoni-test/locations/us-central1/keyRings/telmoni/cryptoKeys/connector-kek",
        )
        .unwrap();
        assert_eq!(kek.kind(), "kms");
        assert!(!kek.is_local());
        assert!(format!("{kek:?}").contains("connector-kek"));
        assert_eq!(local().kind(), "local");
        assert!(local().is_local());
    }

    #[test]
    fn hex_decodes_both_cases_and_refuses_the_rest() {
        assert_eq!(decode_hex("00ff").unwrap().as_slice(), &[0x00, 0xff]);
        assert_eq!(decode_hex("0AfE").unwrap().as_slice(), &[0x0a, 0xfe]);
        assert_eq!(decode_hex("").unwrap().len(), 0);
        assert!(decode_hex("abc").unwrap_err().contains("odd"));
        assert!(decode_hex("zz").unwrap_err().contains("not a hex digit"));
    }

    /// Any pod is a deployed tier; a laptop sets no Kubernetes variable.
    #[test]
    fn a_deployed_tier_is_a_kubernetes_pod() {
        use std::ffi::OsStr;
        assert!(!deployed_from(None), "a laptop sets nothing");
        assert!(
            !deployed_from(Some(OsStr::new(""))),
            "an empty value names no API server"
        );
        assert!(deployed_from(Some(OsStr::new("10.8.0.1"))), "a pod");
    }

    #[test]
    fn loopback_is_the_three_spellings_of_this_machine_and_nothing_else() {
        for origin in [
            "http://localhost:3000",
            "http://LOCALHOST",
            "http://127.0.0.1:3000",
            "http://127.0.0.2",
            "http://[::1]:3000",
        ] {
            assert!(is_loopback_origin(origin), "{origin}");
        }
        for origin in [
            "https://app.example",
            "https://telmoni.com",
            "http://10.0.0.1",
            "http://localhost.example.com",
            "not a url",
            "",
        ] {
            assert!(!is_loopback_origin(origin), "{origin}");
        }
    }

    #[test]
    fn classification_reads_the_body_status_not_the_http_status() {
        let invalid = classify(
            "decrypt",
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"code":400,"message":"Decryption failed","status":"INVALID_ARGUMENT"}}"#,
        );
        assert!(matches!(invalid, KekError::Invalid(_)), "{invalid}");

        // A 400 with no status is a body we did not expect, not a broken row.
        let bare = classify("decrypt", reqwest::StatusCode::BAD_REQUEST, "nope");
        assert!(matches!(bare, KekError::Unavailable(_)), "{bare}");

        let disabled = classify(
            "decrypt",
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":{"code":400,"message":"CryptoKeyVersion is not enabled","status":"FAILED_PRECONDITION"}}"#,
        );
        assert!(matches!(disabled, KekError::Unavailable(_)), "{disabled}");

        let unknown = classify(
            "encrypt",
            reqwest::StatusCode::IM_A_TEAPOT,
            r#"{"error":{"code":418,"message":"short and stout","status":"TEAPOT"}}"#,
        );
        assert!(
            matches!(unknown, KekError::Unavailable(ref m) if m.contains("TEAPOT")),
            "{unknown}"
        );
    }
}
