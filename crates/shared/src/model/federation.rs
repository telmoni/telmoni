//! Model connections that store no key: a project signs in as itself, by a
//! short token this deployment signs, to an AWS role or a Google Cloud pool
//! the customer set to trust it, and is handed credentials that last minutes.
//! Each project is an issuer of its own (`<issuer base>/oidc/v1/projects/<id>`),
//! so no tenant's token can stand for another's — the confused deputy one
//! identity shared by every tenant would be — and nothing that could leak is
//! kept: the sealed field holds whom to become, never a secret.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::{Credentials, Refused};
use crate::envelope::MetadataToken;
use crate::net_guard::Egress;
use crate::{OrganizationId, ProjectId, Redacted};

/// How long a token this deployment signs stands: one exchange's worth.
const TOKEN_SECS: i64 = 300;
/// How long a role's or a pool's credentials are asked for.
const SESSION_SECS: u64 = 900;
/// Credentials are dropped this long before they lapse.
const SESSION_MARGIN: Duration = Duration::from_secs(60);
/// The audience AWS's token exchange takes, as an IAM provider's client id.
pub const AWS_AUDIENCE: &str = "sts.amazonaws.com";
/// What a Google Cloud token is asked for.
const CLOUD_PLATFORM: &str = "https://www.googleapis.com/auth/cloud-platform";
/// A P-256 public key's DER prefix, as Cloud KMS answers one (SPKI).
const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// Whom a connection signs in as, as its sealed field holds it: no secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Federated {
    /// An AWS IAM role, assumed with the project's token.
    #[expect(missing_docs, reason = "the variant says what its field is")]
    AwsRole { role_arn: String },
    /// A Google Cloud workload identity pool's provider
    /// (`projects/<n>/locations/global/workloadIdentityPools/<p>/providers/<r>`),
    /// the project Vertex AI is called in, and the service account the pool
    /// lets it stand in for, where it names one.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    GoogleCloud {
        provider: String,
        project_id: String,
        service_account: Option<String>,
    },
}

/// The configuration as a connection's secret spells it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AwsRoleField {
    role_arn: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoogleCloudField {
    workload_identity_provider: String,
    project_id: String,
    #[serde(default)]
    service_account: Option<String>,
}

impl Federated {
    /// `{"role_arn": …}`, an IAM role's ARN in any partition; `None` where
    /// the secret is no such object, so it is read as a key pair instead.
    ///
    /// # Errors
    ///
    /// An object naming a role whose ARN is no role's.
    pub fn aws_role(secret: &str) -> Option<Result<Self, String>> {
        let field: AwsRoleField = serde_json::from_str(secret).ok()?;
        let arn = field.role_arn.trim();
        let (partition, rest) = arn
            .strip_prefix("arn:")
            .and_then(|r| r.split_once(":iam::"))
            .unwrap_or_default();
        let (account, role) = rest.split_once(":role/").unwrap_or_default();
        // China's partition answers at `amazonaws.com.cn`, which no request
        // here names, so its roles are refused rather than never reached.
        let ok = matches!(partition, "aws" | "aws-us-gov")
            && account.len() == 12
            && account.chars().all(|c| c.is_ascii_digit())
            && (1..=512).contains(&role.len())
            && role
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+=,.@_-/".contains(c));
        Some(if ok {
            Ok(Self::AwsRole {
                role_arn: arn.to_owned(),
            })
        } else {
            Err("a role is its ARN, such as arn:aws:iam::123456789012:role/telmoni-judge".into())
        })
    }

    /// `{"workload_identity_provider": …, "project_id": …, "service_account": …}`;
    /// `None` where the secret is no such object, so it is read as a key.
    ///
    /// # Errors
    ///
    /// A provider, project or service account that is none.
    pub fn google_cloud(secret: &str) -> Option<Result<Self, String>> {
        let field: GoogleCloudField = serde_json::from_str(secret).ok()?;
        let provider = field
            .workload_identity_provider
            .trim()
            .trim_start_matches("//iam.googleapis.com/");
        let parts: Vec<&str> = provider.split('/').collect();
        let named = |s: &str| {
            !s.is_empty()
                && s.len() <= 63
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        };
        let provider_ok = matches!(
            parts.as_slice(),
            ["projects", number, "locations", "global", "workloadIdentityPools", pool, "providers", name]
                if !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) && named(pool) && named(name)
        );
        let project = field.project_id.trim();
        let project_ok = (6..=30).contains(&project.len())
            && project.starts_with(|c: char| c.is_ascii_lowercase())
            && project
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        let account = field
            .service_account
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let account_ok = account.is_none_or(|a| {
            a.split_once('@').is_some_and(|(name, domain)| {
                named(name) && domain.ends_with(".gserviceaccount.com")
            })
        });
        Some(if provider_ok && project_ok && account_ok {
            Ok(Self::GoogleCloud {
                provider: provider.to_owned(),
                project_id: project.to_owned(),
                service_account: account.map(str::to_owned),
            })
        } else {
            Err("a pool is its provider's name (projects/<number>/locations/global/workloadIdentityPools/<pool>/providers/<provider>), with the project Vertex AI runs in and, where it impersonates one, a service account's email".into())
        })
    }

    /// How the console and the owners' notices show it: the role's whole
    /// ARN, its account among it, or the account it stands in for, else the
    /// pool's whole provider, so a role of the same name in another account
    /// never reads as the one before.
    #[must_use]
    pub fn shown(&self) -> String {
        match self {
            Self::AwsRole { role_arn } => role_arn.clone(),
            Self::GoogleCloud {
                provider,
                service_account,
                ..
            } => service_account.clone().unwrap_or_else(|| provider.clone()),
        }
    }
}

/// What signs the projects' tokens: a Cloud KMS key version
/// (`EC_SIGN_P256_SHA256`), or, on a laptop, a P-256 key of its own.
pub enum Signer {
    /// KMS key versions, signing over REST with the metadata server's bearer.
    Kms {
        /// The versions published, the first the one signing: named by the
        /// configuration alone, so a version made or imported into the key by
        /// anyone verifies nothing until a deploy names it.
        versions: Vec<String>,
        /// Cloud KMS's origin.
        api_base: String,
        /// The process's bearer.
        metadata: MetadataToken,
        /// The signing version's public key, read once, for its `kid`.
        jwk: tokio::sync::OnceCell<Value>,
        /// The published versions' keys and when they are read again, so a
        /// version disabled drops out of every key set with no restart; the
        /// last set read stands while KMS does not answer.
        published: std::sync::Mutex<Option<(Instant, Vec<Value>)>>,
        /// Held by the one read of the key set under way.
        reading: tokio::sync::Mutex<()>,
    },
    /// A key of a laptop's own, refused on a deployed tier.
    Local(EcdsaKeyPair),
}

/// Why a version's public key was not read.
enum KeyRead {
    /// KMS answered, and the version verifies nothing: disabled, destroyed,
    /// or not P-256.
    Refused(String),
    /// KMS did not answer.
    Unavailable(String),
}

impl KeyRead {
    fn into_message(self) -> String {
        match self {
            Self::Refused(why) | Self::Unavailable(why) => why,
        }
    }
}

impl Signer {
    /// The signer `value` names: `local:<base64url PKCS#8 P-256 key>` off a
    /// tier, else one to four KMS key versions, comma-separated, the first
    /// signing and every one published, as a rotation names two.
    ///
    /// # Errors
    ///
    /// A value that is neither, or a laptop's key on a tier.
    pub fn parse(
        var: &str,
        value: &Redacted,
        kms_api_base: &str,
        metadata_api_base: &str,
    ) -> Result<Self, String> {
        let value = value.expose().trim();
        if let Some(encoded) = value.strip_prefix("local:") {
            if crate::envelope::on_deployed_tier() {
                return Err(format!(
                    "{var} is a laptop's key in a Kubernetes pod; a tier signs with its KMS key"
                ));
            }
            let der = zeroize::Zeroizing::new(
                URL_SAFE_NO_PAD
                    .decode(encoded)
                    .map_err(|_| format!("{var}'s local key is not base64url"))?,
            );
            return EcdsaKeyPair::from_pkcs8(
                &ECDSA_P256_SHA256_FIXED_SIGNING,
                &der,
                &SystemRandom::new(),
            )
            .map(Self::Local)
            .map_err(|_| format!("{var}'s local key is no P-256 PKCS#8 key"));
        }
        let segment_ok = |s: &str| {
            !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        };
        let versions: Vec<String> = value.split(',').map(|v| v.trim().to_owned()).collect();
        let named = versions.iter().all(|version| {
            matches!(
                version.split('/').collect::<Vec<_>>().as_slice(),
                [
                    "projects",
                    p,
                    "locations",
                    l,
                    "keyRings",
                    r,
                    "cryptoKeys",
                    k,
                    "cryptoKeyVersions",
                    v,
                ] if [p, l, r, k, v].into_iter().all(|s| segment_ok(s))
            )
        });
        let mut distinct = versions.clone();
        distinct.sort_unstable();
        distinct.dedup();
        if !named || versions.len() > 4 || distinct.len() != versions.len() {
            return Err(format!(
                "{var} is neither `local:<key>` nor one to four distinct KMS key versions, \
                 comma-separated, the first signing \
                 (projects/<p>/locations/<l>/keyRings/<r>/cryptoKeys/<k>/cryptoKeyVersions/<n>)"
            ));
        }
        Ok(Self::Kms {
            versions,
            api_base: kms_api_base.trim_end_matches('/').to_owned(),
            metadata: MetadataToken::new(metadata_api_base),
            jwk: tokio::sync::OnceCell::new(),
            published: std::sync::Mutex::new(None),
            reading: tokio::sync::Mutex::new(()),
        })
    }

    /// A fresh laptop key, `local:` and its PKCS#8 in base64url: what
    /// `.env.example` tells a developer to make.
    ///
    /// # Errors
    ///
    /// No randomness.
    pub fn generate_local() -> Result<String, String> {
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
            .map(|der| format!("local:{}", URL_SAFE_NO_PAD.encode(der.as_ref())))
            .map_err(|_| "no randomness for a key".to_owned())
    }

    /// The signing key's public half as a JWK, its `kid` its point's digest.
    async fn jwk(&self, http: &reqwest::Client) -> Result<Value, String> {
        match self {
            Self::Local(key) => Ok(jwk_of(key.public_key().as_ref())),
            Self::Kms {
                versions,
                api_base,
                metadata,
                jwk,
                ..
            } => jwk
                .get_or_try_init(|| async {
                    let bearer = metadata.bearer(http).await.map_err(|e| e.to_string())?;
                    let signing = versions.first().map_or("", String::as_str);
                    public_jwk(http, api_base, metadata, &bearer, signing)
                        .await
                        .map_err(KeyRead::into_message)
                })
                .await
                .cloned(),
        }
    }

    /// Every key a token may verify under: each version the configuration
    /// publishes that KMS still holds enabled; a laptop's one key. One read
    /// at a time, every other caller served the last set meanwhile.
    async fn published(&self, http: &reqwest::Client) -> Result<Vec<Value>, String> {
        const FRESH: Duration = Duration::from_secs(300);
        const RETRY: Duration = Duration::from_secs(30);
        let Self::Kms {
            versions,
            api_base,
            metadata,
            published,
            reading,
            ..
        } = self
        else {
            return Ok(vec![self.jwk(http).await?]);
        };
        let kept = || {
            published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        };
        let keep = |until: Duration, keys: &[Value]| {
            *published
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some((Instant::now() + until, keys.to_vec()));
        };
        if let Some((until, keys)) = kept()
            && Instant::now() < until
        {
            return Ok(keys);
        }
        let _reading = match (reading.try_lock(), kept()) {
            (Ok(held), _) => held,
            (Err(_), Some((_, keys))) => return Ok(keys),
            (Err(_), None) => reading.lock().await,
        };
        let last = kept();
        if let Some((until, keys)) = &last
            && Instant::now() < *until
        {
            return Ok(keys.clone());
        }
        let read = async {
            let bearer = metadata.bearer(http).await.map_err(|e| e.to_string())?;
            let mut keys = Vec::new();
            for version in versions {
                match public_jwk(http, api_base, metadata, &bearer, version).await {
                    Ok(jwk) => keys.push(jwk),
                    // Disabled or destroyed: it verifies nothing from now on.
                    Err(KeyRead::Refused(_)) => {}
                    Err(KeyRead::Unavailable(why)) => return Err(why),
                }
            }
            Ok(keys)
        };
        match (read.await, last) {
            (Ok(keys), _) => {
                keep(FRESH, &keys);
                Ok(keys)
            }
            // An outage is asked again shortly, not by every caller.
            (Err(_), Some((_, keys))) => {
                keep(RETRY, &keys);
                Ok(keys)
            }
            (Err(why), None) => Err(why),
        }
    }

    /// `message` signed, as JOSE takes an ES256 signature: `r` then `s`.
    async fn sign(&self, http: &reqwest::Client, message: &[u8]) -> Result<Vec<u8>, String> {
        match self {
            Self::Local(key) => key
                .sign(&SystemRandom::new(), message)
                .map(|signature| signature.as_ref().to_vec())
                .map_err(|_| "the local key would not sign".to_owned()),
            Self::Kms {
                versions,
                api_base,
                metadata,
                ..
            } => {
                #[derive(Deserialize)]
                struct Signed {
                    signature: String,
                }
                let version = versions.first().map_or("", String::as_str);
                let bearer = metadata.bearer(http).await.map_err(|e| e.to_string())?;
                let digest = STANDARD.encode(Sha256::digest(message));
                let resp = http
                    .post(format!("{api_base}/v1/{version}:asymmetricSign"))
                    .bearer_auth(bearer.expose())
                    .json(&json!({ "digest": { "sha256": digest } }))
                    .send()
                    .await
                    .map_err(|e| format!("kms asymmetricSign unreachable: {}", e.without_url()))?;
                if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                    metadata.forget().await;
                }
                let signed: Signed = resp
                    .error_for_status()
                    .map_err(|e| format!("kms asymmetricSign refused: {}", e.without_url()))?
                    .json()
                    .await
                    .map_err(|_| "kms asymmetricSign answer unreadable".to_owned())?;
                let der = STANDARD
                    .decode(signed.signature)
                    .map_err(|_| "kms signature is not base64".to_owned())?;
                fixed_of(&der).ok_or_else(|| "kms signature is no ECDSA signature".to_owned())
            }
        }
    }
}

/// One KMS key version's public key as a JWK; refused unless it is P-256.
async fn public_jwk(
    http: &reqwest::Client,
    api_base: &str,
    metadata: &MetadataToken,
    bearer: &Redacted,
    version: &str,
) -> Result<Value, KeyRead> {
    #[derive(Deserialize)]
    struct PublicKey {
        pem: String,
        algorithm: String,
    }
    let resp = http
        .get(format!("{api_base}/v1/{version}/publicKey"))
        .bearer_auth(bearer.expose())
        .send()
        .await
        .map_err(|e| {
            KeyRead::Unavailable(format!("kms publicKey unreachable: {}", e.without_url()))
        })?;
    let status = resp.status();
    // A version disabled or destroyed is refused as the request it is; a
    // bearer refused, a quota spent or anything else is KMS's state, which
    // keeps the last set read.
    match status.as_u16() {
        400 | 404 => return Err(KeyRead::Refused(format!("kms publicKey answered {status}"))),
        401 => {
            metadata.forget().await;
            return Err(KeyRead::Unavailable(format!(
                "kms publicKey answered {status}"
            )));
        }
        _ => {}
    }
    let answer: PublicKey = resp
        .error_for_status()
        .map_err(|e| KeyRead::Unavailable(format!("kms publicKey failed: {}", e.without_url())))?
        .json()
        .await
        .map_err(|_| KeyRead::Unavailable("kms publicKey answer unreadable".to_owned()))?;
    if answer.algorithm != "EC_SIGN_P256_SHA256" {
        return Err(KeyRead::Refused(format!(
            "the signing key is {}, not EC_SIGN_P256_SHA256",
            answer.algorithm
        )));
    }
    let body: String = answer
        .pem
        .lines()
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = STANDARD
        .decode(body)
        .map_err(|_| KeyRead::Refused("kms publicKey is not PEM".to_owned()))?;
    let point = der
        .strip_prefix(P256_SPKI_PREFIX.as_slice())
        .filter(|p| p.len() == 65)
        .ok_or_else(|| KeyRead::Refused("kms publicKey is no P-256 key".to_owned()))?;
    Ok(jwk_of(point))
}

/// A P-256 point (`04 ‖ x ‖ y`) as a signing JWK.
fn jwk_of(point: &[u8]) -> Value {
    let (x, y) = point.get(1..33).zip(point.get(33..65)).unwrap_or_default();
    json!({
        "kty": "EC", "crv": "P-256", "alg": "ES256", "use": "sig",
        "kid": kid_of(point),
        "x": URL_SAFE_NO_PAD.encode(x), "y": URL_SAFE_NO_PAD.encode(y),
    })
}

/// A key's id: its point's digest, so a token's `kid` and its JWK's agree
/// whichever signer made them.
fn kid_of(point: &[u8]) -> String {
    let digest = Sha256::digest(point);
    crate::digest::hex_lower(digest.get(..8).unwrap_or_default())
}

/// An ASN.1 ECDSA signature (`SEQUENCE { INTEGER r, INTEGER s }`) as 64 bytes.
fn fixed_of(der: &[u8]) -> Option<Vec<u8>> {
    let (&tag, rest) = der.split_first()?;
    let (&len, mut body) = rest.split_first()?;
    if tag != 0x30 || usize::from(len) != body.len() {
        return None;
    }
    let mut fixed = Vec::with_capacity(64);
    for _ in 0..2 {
        let (&tag, rest) = body.split_first()?;
        let (&len, rest) = rest.split_first()?;
        let len = usize::from(len);
        if tag != 0x02 || rest.len() < len {
            return None;
        }
        let (int, after) = rest.split_at(len);
        let int = int.strip_prefix(&[0]).unwrap_or(int);
        if int.len() > 32 {
            return None;
        }
        fixed.extend(std::iter::repeat_n(0, 32 - int.len()));
        fixed.extend_from_slice(int);
        body = after;
    }
    body.is_empty().then_some(fixed)
}

/// Where the exchanges are spoken to; the tests point them at mocks.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// AWS STS's origin; `None` is the connection's region's own.
    pub aws_sts: Option<String>,
    /// Google's Security Token Service.
    pub google_sts: String,
    /// Google's IAM credentials API, where a pool's token becomes an account's.
    pub google_iam: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            aws_sts: None,
            google_sts: "https://sts.googleapis.com".into(),
            google_iam: "https://iamcredentials.googleapis.com".into(),
        }
    }
}

/// Credentials handed by an exchange, and when to stop using them.
struct Session {
    live: Credentials,
    until: Instant,
}

/// The deployment as an issuer: the signer, where its issuers are reached,
/// and the credentials each connection was last handed.
pub struct Federation {
    signer: Signer,
    issuer_base: String,
    http: reqwest::Client,
    endpoints: Endpoints,
    sessions: tokio::sync::Mutex<HashMap<[u8; 32], Session>>,
}

impl std::fmt::Debug for Federation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Federation")
            .field("issuer_base", &self.issuer_base)
            .finish_non_exhaustive()
    }
}

impl Federation {
    /// The issuer at `issuer_base` (the machine host's origin), signing with
    /// `signer` and calling KMS through `http`.
    #[must_use]
    pub fn new(
        signer: Signer,
        issuer_base: &str,
        http: reqwest::Client,
        endpoints: Endpoints,
    ) -> Self {
        Self {
            signer,
            issuer_base: issuer_base.trim_end_matches('/').to_owned(),
            http,
            endpoints,
            sessions: tokio::sync::Mutex::default(),
        }
    }

    /// Project `project`'s issuer: the URL a customer's provider trusts.
    #[must_use]
    pub fn issuer(&self, project: &ProjectId) -> String {
        format!("{}/oidc/v1/projects/{project}", self.issuer_base)
    }

    /// The subject a project's tokens name, its organization's with it: a
    /// project moved to another organization names another, so a role or a
    /// pool its old one set to trust it trusts it no more.
    #[must_use]
    pub fn subject(organization: &OrganizationId, project: &ProjectId) -> String {
        format!("organization:{organization}:project:{project}")
    }

    /// The JWKS every project's issuer names.
    #[must_use]
    pub fn jwks_uri(&self) -> String {
        format!("{}/oidc/v1/jwks", self.issuer_base)
    }

    /// Project `project`'s OpenID discovery document.
    #[must_use]
    pub fn discovery(&self, project: &ProjectId) -> Value {
        json!({
            "issuer": self.issuer(project),
            "jwks_uri": self.jwks_uri(),
            "response_types_supported": ["id_token"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["ES256"],
            "claims_supported": ["iss", "sub", "aud", "iat", "nbf", "exp", "jti"],
        })
    }

    /// The key set the tokens verify under.
    ///
    /// # Errors
    ///
    /// The signer's key could not be read.
    pub async fn jwks(&self) -> Result<Value, String> {
        Ok(json!({ "keys": self.signer.published(&self.http).await? }))
    }

    /// A token naming `project`, for `audience`, valid for minutes.
    ///
    /// # Errors
    ///
    /// The signer could not sign.
    pub async fn token(
        &self,
        organization: &OrganizationId,
        project: &ProjectId,
        audience: &str,
    ) -> Result<String, String> {
        let jwk = self.signer.jwk(&self.http).await?;
        let now = chrono::Utc::now().timestamp();
        let header = json!({ "alg": "ES256", "typ": "JWT", "kid": jwk.get("kid") });
        let claims = json!({
            "iss": self.issuer(project), "sub": Self::subject(organization, project), "aud": audience,
            "iat": now, "nbf": now, "exp": now + TOKEN_SECS, "jti": Uuid::new_v4(),
        });
        let input = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature = self.signer.sign(&self.http, input.as_bytes()).await?;
        Ok(format!("{input}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }

    /// Credentials `federated` is handed for connection `connection` of
    /// `project`: a role's session or a pool's token, kept until a minute
    /// before it lapses so a busy connection signs in a few times an hour.
    ///
    /// # Errors
    ///
    /// The provider refused the project ([`Refused::Unauthorized`], which
    /// pauses its evaluators), limited it, or could not be reached.
    pub async fn sign_in(
        &self,
        egress: &Egress,
        organization: &OrganizationId,
        project: &ProjectId,
        connection: Uuid,
        federated: &Federated,
        region: Option<&str>,
    ) -> Result<Credentials, Refused> {
        let key: [u8; 32] = Sha256::digest(
            format!(
                "{connection}\0{organization}\0{project}\0{federated:?}\0{}",
                region.unwrap_or_default()
            )
            .as_bytes(),
        )
        .into();
        // Held only to read and to write, never across an exchange: one slow
        // provider waits on its own connection alone.
        {
            let mut sessions = self.sessions.lock().await;
            let now = Instant::now();
            sessions.retain(|_, session| session.until > now);
            if let Some(session) = sessions.get(&key) {
                return Ok(session.live.clone());
            }
        }
        let asked_at = Instant::now();
        let unsigned = |why: String| {
            Refused::Unreached(format!(
                "this deployment could not sign the project's token: {why}"
            ))
        };
        let live = match federated {
            Federated::AwsRole { role_arn } => {
                let region = region.ok_or_else(|| {
                    Refused::Unreadable("a Bedrock connection names its region".into())
                })?;
                let token = self
                    .token(organization, project, AWS_AUDIENCE)
                    .await
                    .map_err(unsigned)?;
                self.assume_role(egress, role_arn, region, connection, &token)
                    .await?
            }
            Federated::GoogleCloud {
                provider,
                project_id,
                service_account,
            } => {
                let token = self
                    .token(
                        organization,
                        project,
                        &format!("https://iam.googleapis.com/{provider}"),
                    )
                    .await
                    .map_err(unsigned)?;
                let (bearer, lasts) = self
                    .pool_token(egress, provider, service_account.as_deref(), &token)
                    .await?;
                (
                    Credentials::GoogleToken {
                        token: bearer,
                        project_id: project_id.clone(),
                    },
                    lasts,
                )
            }
        };
        let (live, lasts) = live;
        // Kept as long as its issuer says it lasts, fifteen minutes at most: a
        // pool's token without an account to stand in for lasts as long as
        // the project's own, five minutes.
        let until = asked_at
            + lasts
                .min(Duration::from_secs(SESSION_SECS))
                .saturating_sub(SESSION_MARGIN);
        self.sessions.lock().await.insert(
            key,
            Session {
                live: live.clone(),
                until,
            },
        );
        Ok(live)
    }

    /// AWS's `AssumeRoleWithWebIdentity`: a role's session for the token.
    async fn assume_role(
        &self,
        egress: &Egress,
        role_arn: &str,
        region: &str,
        connection: Uuid,
        token: &str,
    ) -> Result<(Credentials, Duration), Refused> {
        let base = self
            .endpoints
            .aws_sts
            .clone()
            .unwrap_or_else(|| format!("https://sts.{region}.amazonaws.com"));
        let session = format!("telmoni-{}", connection.simple());
        let duration = SESSION_SECS.to_string();
        let form = [
            ("Action", "AssumeRoleWithWebIdentity"),
            ("Version", "2011-06-15"),
            ("RoleArn", role_arn),
            ("RoleSessionName", session.as_str()),
            ("WebIdentityToken", token),
            ("DurationSeconds", duration.as_str()),
        ];
        let resp = egress
            .post(&format!("{base}/"))
            .map_err(|r| Refused::Unreached(r.reason().into()))?
            .form(&form)
            .send()
            .await
            .map_err(|_| Refused::Unreached("AWS's token service did not answer".into()))?;
        let status = resp.status().as_u16();
        let retry_after = crate::http::retry_after(resp.headers());
        let body = String::from_utf8_lossy(&super::capped(resp, super::TOKEN_ANSWER_MAX).await?)
            .into_owned();
        if status == 200 {
            let part = |tag: &str| {
                xml_text(&body, tag)
                    .ok_or_else(|| Refused::Unreadable("AWS answered no session".into()))
            };
            let lasts = xml_text(&body, "Expiration")
                .and_then(|at| lasts_until(&at))
                .unwrap_or(Duration::from_secs(SESSION_SECS));
            return Ok((
                Credentials::AccessKeys {
                    access_key_id: part("AccessKeyId")?,
                    secret_access_key: part("SecretAccessKey")?.into(),
                    session_token: Some(part("SessionToken")?.into()),
                },
                lasts,
            ));
        }
        let detail = xml_text(&body, "Message").unwrap_or_else(|| format!("status {status}"));
        let code = xml_text(&body, "Code").unwrap_or_default();
        // How AWS words a key set it could not read: the deployment's state.
        let unread = ["verification key", "retrieve"]
            .iter()
            .any(|word| detail.to_ascii_lowercase().contains(word));
        Err(match (status, code.as_str()) {
            (_, "Throttling" | "ThrottlingException" | "RequestLimitExceeded") | (429, _) => {
                Refused::RateLimited { retry_after }
            }
            // AWS could not read the project's issuer, or the token lapsed on
            // its way: this deployment's state, not the customer's trust, so
            // tried again rather than pausing the connection.
            (_, "IDPCommunicationError" | "ExpiredToken" | "ExpiredTokenException")
            | (500..=599, _) => {
                Refused::Unreached(format!("AWS could not take the project's token: {detail}"))
            }
            (_, "InvalidIdentityToken") if unread => {
                Refused::Unreached(format!("AWS could not take the project's token: {detail}"))
            }
            (400 | 403, _) => Refused::Unauthorized {
                status,
                detail: format!("AWS refused the role: {detail}"),
            },
            _ => Refused::Provider { status, detail },
        })
    }

    /// Google's token exchange for the pool, then, where the pool stands in
    /// for an account, that account's token.
    async fn pool_token(
        &self,
        egress: &Egress,
        provider: &str,
        service_account: Option<&str>,
        token: &str,
    ) -> Result<(Redacted, Duration), Refused> {
        let exchanged = google_call(
            egress,
            &format!("{}/v1/token", self.endpoints.google_sts),
            None,
            &json!({
                "grantType": "urn:ietf:params:oauth:grant-type:token-exchange",
                "audience": format!("//iam.googleapis.com/{provider}"),
                "scope": CLOUD_PLATFORM,
                "requestedTokenType": "urn:ietf:params:oauth:token-type:access_token",
                "subjectToken": token,
                "subjectTokenType": "urn:ietf:params:oauth:token-type:jwt",
            }),
            "/access_token",
        )
        .await?;
        let Some(account) = service_account else {
            return Ok(exchanged);
        };
        google_call(
            egress,
            &format!(
                "{}/v1/projects/-/serviceAccounts/{}:generateAccessToken",
                self.endpoints.google_iam,
                super::sigv4::encode_segment(account)
            ),
            Some(&exchanged.0),
            &json!({ "scope": [CLOUD_PLATFORM], "lifetime": format!("{SESSION_SECS}s") }),
            "/accessToken",
        )
        .await
    }
}

/// One of Google's token calls: its token at `pointer`, and how long it
/// lasts, by its `expires_in` or its `expireTime`, else as long as the
/// project's own token.
async fn google_call(
    egress: &Egress,
    url: &str,
    bearer: Option<&Redacted>,
    body: &Value,
    pointer: &str,
) -> Result<(Redacted, Duration), Refused> {
    let mut request = egress
        .post(url)
        .map_err(|r| Refused::Unreached(r.reason().into()))?
        .json(body);
    if let Some(bearer) = bearer {
        let mut value =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", bearer.expose()))
                .map_err(|_| {
                    Refused::Unreadable("Google answered a token no header carries".into())
                })?;
        value.set_sensitive(true);
        request = request.header(reqwest::header::AUTHORIZATION, value);
    }
    let resp = request
        .send()
        .await
        .map_err(|_| Refused::Unreached("Google's token service did not answer".into()))?;
    let status = resp.status().as_u16();
    let retry_after = crate::http::retry_after(resp.headers());
    let answer: Value =
        serde_json::from_slice(&super::capped(resp, super::TOKEN_ANSWER_MAX).await?)
            .unwrap_or(Value::Null);
    if status == 200 {
        let lasts = answer
            .get("expires_in")
            .and_then(Value::as_u64)
            .map(Duration::from_secs)
            .or_else(|| {
                answer
                    .get("expireTime")
                    .and_then(Value::as_str)
                    .and_then(lasts_until)
            })
            .unwrap_or(Duration::from_secs(TOKEN_SECS.unsigned_abs()));
        return answer
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(|token| (Redacted::from(token.to_owned()), lasts))
            .ok_or_else(|| Refused::Unreadable("Google answered no token".into()));
    }
    let detail = answer
        .pointer("/error_description")
        .or_else(|| answer.pointer("/error/message"))
        .and_then(Value::as_str)
        .unwrap_or("no reason given")
        .to_owned();
    // Google could not read the project's issuer: this deployment's state,
    // not the customer's trust, so tried again rather than pausing.
    let unread = ["jwk", "fetch", "retrieve"]
        .iter()
        .any(|word| detail.to_ascii_lowercase().contains(word));
    Err(match status {
        429 => Refused::RateLimited { retry_after },
        500..=599 => Refused::Unreached(format!("Google's token service answered {status}")),
        400 if unread => Refused::Unreached(format!(
            "Google could not read the project's issuer: {detail}"
        )),
        400 | 401 | 403 => Refused::Unauthorized {
            status,
            detail: format!("Google refused the pool: {detail}"),
        },
        _ => Refused::Provider { status, detail },
    })
}

/// How long until an RFC 3339 time, none where it is past or unreadable.
fn lasts_until(at: &str) -> Option<Duration> {
    let at = chrono::DateTime::parse_from_rfc3339(at.trim()).ok()?;
    (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
        .to_std()
        .ok()
}

/// The text between `<tag>` and `</tag>`, its five entities read.
fn xml_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let start = body.find(&open)? + open.len();
    let end = start + body.get(start..)?.find(&format!("</{tag}>"))?;
    Some(
        body.get(start..end)?
            .trim()
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

#[cfg(test)]
mod tests {
    use std::future::IntoFuture as _;

    use axum::Router;
    use axum::routing::post;
    use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

    use super::*;
    use crate::OrganizationId;

    fn local() -> Signer {
        let key = Signer::generate_local().expect("a key");
        Signer::parse(
            "SIGNER",
            &Redacted::from(key),
            "https://kms",
            "http://metadata",
        )
        .expect("parsed")
    }

    fn federation(endpoints: Endpoints) -> Federation {
        Federation::new(
            local(),
            "https://api.example/",
            reqwest::Client::new(),
            endpoints,
        )
    }

    /// A token names its project's issuer and subject, and verifies under
    /// the one key the JWKS publishes, by the `kid` it names.
    #[tokio::test]
    async fn a_token_verifies_under_its_issuers_key_set() {
        let federation = federation(Endpoints::default());
        let (organization, project) = (OrganizationId::new(), ProjectId::new());
        let token = federation
            .token(&organization, &project, AWS_AUDIENCE)
            .await
            .expect("signed");
        let (input, signature) = token.rsplit_once('.').expect("three parts");
        let (header, claims) = input.split_once('.').expect("three parts");
        let read = |part: &str| -> Value {
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(part).expect("base64url")).expect("json")
        };
        let (header, claims) = (read(header), read(claims));
        assert_eq!(
            claims["iss"],
            format!("https://api.example/oidc/v1/projects/{project}")
        );
        assert_eq!(
            claims["sub"],
            format!("organization:{organization}:project:{project}")
        );
        assert_eq!(claims["aud"], AWS_AUDIENCE);
        assert_eq!(
            claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
            TOKEN_SECS
        );
        let jwks = federation.jwks().await.expect("the key set");
        let jwk = &jwks["keys"][0];
        assert_eq!(header["kid"], jwk["kid"]);
        let point: Vec<u8> = [
            vec![4],
            URL_SAFE_NO_PAD.decode(jwk["x"].as_str().unwrap()).unwrap(),
            URL_SAFE_NO_PAD.decode(jwk["y"].as_str().unwrap()).unwrap(),
        ]
        .concat();
        UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, &point)
            .verify(
                input.as_bytes(),
                &URL_SAFE_NO_PAD.decode(signature).unwrap(),
            )
            .expect("verifies");
        let discovery = federation.discovery(&project);
        assert_eq!(discovery["issuer"], claims["iss"]);
        assert_eq!(discovery["jwks_uri"], "https://api.example/oidc/v1/jwks");
    }

    /// The key set publishes the versions the configuration names and no
    /// other, drops one KMS answers as disabled, and keeps the last set read
    /// while KMS does not answer, asking again shortly rather than each time.
    #[tokio::test]
    async fn the_key_set_is_the_named_versions_kms_still_holds() {
        use axum::routing::get;
        let pem = |point: &[u8]| {
            let der = [P256_SPKI_PREFIX.as_slice(), point].concat();
            format!(
                "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
                STANDARD.encode(der)
            )
        };
        let key = || {
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(
                &ECDSA_P256_SHA256_FIXED_SIGNING,
                &SystemRandom::new(),
            )
            .unwrap();
            let pair = EcdsaKeyPair::from_pkcs8(
                &ECDSA_P256_SHA256_FIXED_SIGNING,
                pkcs8.as_ref(),
                &SystemRandom::new(),
            )
            .unwrap();
            pair.public_key().as_ref().to_vec()
        };
        let (one, two) = (key(), key());
        let answers = std::sync::Arc::new(std::sync::Mutex::new((pem(&one), true)));
        let kept = answers.clone();
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        let (one_pem, two_pem) = (pem(&one), pem(&two));
        let app = Router::new()
            .route(
                "/computeMetadata/v1/instance/service-accounts/default/token",
                get(|| async { axum::Json(json!({ "access_token": "t", "expires_in": 3600 })) }),
            )
            .route(
                "/v1/projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/{version}/publicKey",
                get(move |axum::extract::Path(version): axum::extract::Path<String>| {
                    let (kept, one_pem, two_pem) = (kept.clone(), one_pem.clone(), two_pem.clone());
                    counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async move {
                        let up = kept.lock().unwrap().1;
                        let pem = if version == "1" { one_pem } else { two_pem };
                        match (up, version.as_str()) {
                            (false, _) => (axum::http::StatusCode::SERVICE_UNAVAILABLE, String::new()),
                            (true, "3") => (axum::http::StatusCode::BAD_REQUEST, "FAILED_PRECONDITION".into()),
                            (true, _) => (
                                axum::http::StatusCode::OK,
                                json!({ "pem": pem, "algorithm": "EC_SIGN_P256_SHA256" }).to_string(),
                            ),
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(axum::serve(listener, app).into_future());
        let named = "projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions";
        let signer = Signer::parse(
            "SIGNER",
            &Redacted::from(format!("{named}/1, {named}/2,{named}/3")),
            &base,
            &base,
        )
        .expect("three versions");
        let http = reqwest::Client::new();
        let keys = signer.published(&http).await.expect("the key set");
        let kids: Vec<&str> = keys.iter().filter_map(|k| k["kid"].as_str()).collect();
        assert_eq!(
            kids,
            [kid_of(&one), kid_of(&two)],
            "the disabled third verifies nothing"
        );
        if let Signer::Kms { published, .. } = &signer
            && let Some((until, _)) = published.lock().unwrap().as_mut()
        {
            *until = Instant::now();
        }
        answers.lock().unwrap().1 = false;
        let before = asked.load(std::sync::atomic::Ordering::SeqCst);
        let stale = signer.published(&http).await.expect("the last set read");
        assert_eq!(stale.len(), 2, "an outage keeps the last set read");
        let again = signer.published(&http).await.expect("the last set read");
        assert_eq!(again.len(), 2);
        assert_eq!(
            asked.load(std::sync::atomic::Ordering::SeqCst) - before,
            1,
            "an outage was asked again by the next caller"
        );
        for refused in [
            format!("{named}/1,{named}/1"),
            format!("{named}/1,projects/p"),
            [1, 2, 3, 4, 5].map(|n| format!("{named}/{n}")).join(","),
        ] {
            assert!(
                Signer::parse("SIGNER", &Redacted::from(refused.clone()), &base, &base).is_err(),
                "{refused}"
            );
        }
    }

    /// A KMS signer has the first version it names sign the token's digest,
    /// by the process's bearer, and sends the DER answer as `r` then `s`.
    #[tokio::test]
    async fn a_kms_signer_signs_the_tokens_digest() {
        use axum::routing::get;
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
            &SystemRandom::new(),
        )
        .unwrap();
        let der = [P256_SPKI_PREFIX.as_slice(), pair.public_key().as_ref()].concat();
        let pem = format!(
            "-----BEGIN PUBLIC KEY-----\n{}\n-----END PUBLIC KEY-----\n",
            STANDARD.encode(der)
        );
        let (r, s) = ([0x11_u8; 32], [0x22_u8; 32]);
        let signature = STANDARD.encode(
            [
                vec![0x30, 68, 0x02, 32],
                r.to_vec(),
                vec![0x02, 32],
                s.to_vec(),
            ]
            .concat(),
        );
        let asked = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = asked.clone();
        let app = Router::new()
            .route(
                "/computeMetadata/v1/instance/service-accounts/default/token",
                get(|| async { axum::Json(json!({ "access_token": "t", "expires_in": 3600 })) }),
            )
            .route(
                "/v1/projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/{version}/publicKey",
                get(move || {
                    let pem = pem.clone();
                    async move { axum::Json(json!({ "pem": pem, "algorithm": "EC_SIGN_P256_SHA256" })) }
                }),
            )
            .route(
                "/v1/projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions/{version}",
                post(
                    move |axum::extract::Path(version): axum::extract::Path<String>,
                          headers: axum::http::HeaderMap,
                          axum::Json(body): axum::Json<Value>| {
                        let (kept, signature) = (kept.clone(), signature.clone());
                        async move {
                            let bearer = headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_owned();
                            kept.lock().unwrap().push((version, bearer, body));
                            axum::Json(json!({ "signature": signature }))
                        }
                    },
                ),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(axum::serve(listener, app).into_future());
        let named = "projects/p/locations/l/keyRings/r/cryptoKeys/k/cryptoKeyVersions";
        let signer = Signer::parse(
            "SIGNER",
            &Redacted::from(format!("{named}/2,{named}/1")),
            &base,
            &base,
        )
        .expect("two versions");
        let federation = Federation::new(
            signer,
            "https://api.example/",
            reqwest::Client::new(),
            Endpoints::default(),
        );
        let token = federation
            .token(&OrganizationId::new(), &ProjectId::new(), AWS_AUDIENCE)
            .await
            .expect("signed");
        let (input, sent) = token.rsplit_once('.').expect("a signed token");
        assert_eq!(URL_SAFE_NO_PAD.decode(sent).unwrap(), [r, s].concat());
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.len(), 1);
        let (version, bearer, body) = &asked[0];
        assert_eq!(version, "2:asymmetricSign", "the first version named signs");
        assert_eq!(bearer, "Bearer t");
        assert_eq!(
            body["digest"]["sha256"],
            STANDARD.encode(Sha256::digest(input.as_bytes()))
        );
    }

    /// Google refusing a pool pauses as a refused key does; its limit waits;
    /// failing to read the issuer, or failing itself, is tried again.
    #[tokio::test]
    async fn googles_answers_are_told_apart() {
        let egress = Egress::unguarded(reqwest::Client::new());
        let pool = Federated::GoogleCloud {
            provider: "projects/123/locations/global/workloadIdentityPools/p/providers/r".into(),
            project_id: "acme-ml".into(),
            service_account: None,
        };
        for (status, answer, expected) in [
            (
                400,
                r#"{"error": "invalid_grant", "error_description": "The audience in ID Token does not match"}"#,
                "refused",
            ),
            (
                403,
                r#"{"error": {"message": "Permission denied"}}"#,
                "refused",
            ),
            (
                400,
                r#"{"error": "invalid_grant", "error_description": "Unable to fetch the JWK set"}"#,
                "tried again",
            ),
            (429, "{}", "limited"),
            (503, "", "tried again"),
            (418, "{}", "failed"),
        ] {
            let (sts, _) = mock(status, answer).await;
            let federation = federation(Endpoints {
                aws_sts: None,
                google_sts: sts,
                google_iam: "http://127.0.0.1:1".into(),
            });
            let refused = federation
                .sign_in(
                    &egress,
                    &OrganizationId::new(),
                    &ProjectId::new(),
                    Uuid::new_v4(),
                    &pool,
                    None,
                )
                .await;
            let told = match &refused {
                Err(Refused::Unauthorized { .. }) => "refused",
                Err(Refused::Unreached(_)) => "tried again",
                Err(Refused::RateLimited { .. }) => "limited",
                Err(Refused::Provider { .. }) => "failed",
                other => panic!("{status}: {other:?}"),
            };
            assert_eq!(told, expected, "{status} {answer}");
        }
    }

    /// A KMS signature, DER, is the 64 bytes JOSE takes; anything else is refused.
    #[test]
    fn a_der_signature_is_read_as_r_then_s() {
        let r = [0x80; 32];
        let s = [0x01; 31];
        let der = [
            vec![0x30, 70, 0x02, 33, 0x00],
            r.to_vec(),
            vec![0x02, 31],
            s.to_vec(),
        ]
        .concat();
        let der = [
            der[..1].to_vec(),
            vec![u8::try_from(der.len() - 2).unwrap()],
            der[2..].to_vec(),
        ]
        .concat();
        let fixed = fixed_of(&der).expect("read");
        assert_eq!(&fixed[..32], &r);
        assert_eq!(fixed[32], 0);
        assert_eq!(&fixed[33..], &s);
        assert!(fixed_of(&[0x30, 0x02, 0x05, 0x00]).is_none());
    }

    /// A role is its ARN, a pool its provider's name; anything else is no
    /// configuration, and a key is read where the shape is a key's.
    #[test]
    fn a_role_and_a_pool_are_held_to_their_shapes() {
        assert_eq!(
            Federated::aws_role(r#"{"role_arn": "arn:aws:iam::123456789012:role/judges/telmoni"}"#),
            Some(Ok(Federated::AwsRole {
                role_arn: "arn:aws:iam::123456789012:role/judges/telmoni".into()
            }))
        );
        assert!(matches!(
            Federated::aws_role(r#"{"role_arn": "arn:aws:iam::1234:role/x"}"#),
            Some(Err(_))
        ));
        assert!(
            matches!(
                Federated::aws_role(r#"{"role_arn": "arn:aws-cn:iam::123456789012:role/x"}"#),
                Some(Err(_))
            ),
            "China's partition is no host a request here names"
        );
        assert!(
            Federated::aws_role(r#"{"access_key_id": "AKIA", "secret_access_key": "s"}"#).is_none()
        );
        let pool = Federated::google_cloud(
            r#"{"workload_identity_provider": "projects/123/locations/global/workloadIdentityPools/telmoni/providers/judge", "project_id": "acme-ml", "service_account": "judge@acme-ml.iam.gserviceaccount.com"}"#,
        );
        assert!(
            matches!(pool, Some(Ok(Federated::GoogleCloud { .. }))),
            "{pool:?}"
        );
        assert_eq!(
            pool.unwrap().unwrap().shown(),
            "judge@acme-ml.iam.gserviceaccount.com"
        );
        assert!(matches!(
            Federated::google_cloud(
                r#"{"workload_identity_provider": "projects/x/pools", "project_id": "acme-ml"}"#
            ),
            Some(Err(_))
        ));
        assert!(Federated::google_cloud(r#"{"type": "service_account"}"#).is_none());
    }

    /// A mock of a token service, answering `answer` to every POST and keeping what it was sent.
    async fn mock(
        status: u16,
        answer: &'static str,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = seen.clone();
        let app = Router::new().fallback(post(move |body: String| {
            let kept = kept.clone();
            async move {
                kept.lock().unwrap().push(body);
                (axum::http::StatusCode::from_u16(status).unwrap(), answer)
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a port");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        tokio::spawn(axum::serve(listener, app).into_future());
        (base, seen)
    }

    /// A role's session is read from AWS's answer and kept for the next
    /// judgement; a refusal of the role pauses as a refused key does.
    #[tokio::test]
    async fn a_role_is_assumed_once_and_its_refusal_is_the_keys() {
        let (base, seen) = mock(
            200,
            "<AssumeRoleWithWebIdentityResponse><AssumeRoleWithWebIdentityResult><Credentials>\
             <AccessKeyId>ASIAEXAMPLE</AccessKeyId><SecretAccessKey>s3cr3t</SecretAccessKey>\
             <SessionToken>t0k&amp;en</SessionToken><Expiration>2099-01-01T00:00:00Z</Expiration>\
             </Credentials></AssumeRoleWithWebIdentityResult></AssumeRoleWithWebIdentityResponse>",
        )
        .await;
        let federation = federation(Endpoints {
            aws_sts: Some(base),
            ..Endpoints::default()
        });
        let egress = Egress::unguarded(reqwest::Client::new());
        let (organization, project, connection) =
            (OrganizationId::new(), ProjectId::new(), Uuid::new_v4());
        let role = Federated::AwsRole {
            role_arn: "arn:aws:iam::123456789012:role/telmoni".into(),
        };
        for _ in 0..2 {
            let live = federation
                .sign_in(
                    &egress,
                    &organization,
                    &project,
                    connection,
                    &role,
                    Some("us-east-1"),
                )
                .await
                .expect("signed in");
            assert!(matches!(
                &live,
                Credentials::AccessKeys { access_key_id, secret_access_key, session_token: Some(token) }
                    if access_key_id == "ASIAEXAMPLE" && secret_access_key.expose() == "s3cr3t" && token.expose() == "t0k&en"
            ));
        }
        let sent = seen.lock().unwrap().clone();
        assert_eq!(sent.len(), 1, "the session was kept: {sent:?}");
        assert!(
            sent[0].contains("Action=AssumeRoleWithWebIdentity")
                && sent[0].contains("WebIdentityToken=ey")
        );

        let (base, _) = mock(403, "<ErrorResponse><Error><Code>AccessDenied</Code><Message>Not authorized to perform sts:AssumeRoleWithWebIdentity</Message></Error></ErrorResponse>").await;
        let refused = federation_with(base)
            .sign_in(
                &egress,
                &organization,
                &project,
                connection,
                &role,
                Some("us-east-1"),
            )
            .await;
        assert!(
            matches!(refused, Err(Refused::Unauthorized { status: 403, ref detail }) if detail.contains("Not authorized")),
            "{refused:?}"
        );

        // AWS failing to read the issuer is tried again, never a refused role.
        let (base, _) = mock(400, "<ErrorResponse><Error><Code>InvalidIdentityToken</Code><Message>Couldn't retrieve verification key from your identity provider, please reference AssumeRoleWithWebIdentity documentation for requirements</Message></Error></ErrorResponse>").await;
        let retried = federation_with(base)
            .sign_in(
                &egress,
                &organization,
                &project,
                connection,
                &role,
                Some("us-east-1"),
            )
            .await;
        assert!(matches!(retried, Err(Refused::Unreached(_))), "{retried:?}");
    }

    fn federation_with(aws: String) -> Federation {
        federation(Endpoints {
            aws_sts: Some(aws),
            ..Endpoints::default()
        })
    }

    /// A pool's own token is kept no longer than Google says it lasts, which
    /// is as long as the project's token: never past its expiry.
    #[tokio::test]
    async fn a_pool_token_is_kept_no_longer_than_it_lasts() {
        let (sts, seen) = mock(200, r#"{"access_token": "federated", "expires_in": 45}"#).await;
        let federation = federation(Endpoints {
            aws_sts: None,
            google_sts: sts,
            google_iam: "http://127.0.0.1:1".into(),
        });
        let egress = Egress::unguarded(reqwest::Client::new());
        let pool = Federated::GoogleCloud {
            provider: "projects/123/locations/global/workloadIdentityPools/p/providers/r".into(),
            project_id: "acme-ml".into(),
            service_account: None,
        };
        let (organization, project, connection) =
            (OrganizationId::new(), ProjectId::new(), Uuid::new_v4());
        for _ in 0..2 {
            federation
                .sign_in(&egress, &organization, &project, connection, &pool, None)
                .await
                .expect("signed in");
        }
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "a token lasting under the margin is never kept"
        );
    }

    /// A pool's token is exchanged, then, where it stands in for an account,
    /// traded for that account's.
    #[tokio::test]
    async fn a_pool_token_becomes_an_accounts() {
        let (sts, sts_seen) =
            mock(200, r#"{"access_token": "federated", "expires_in": 3600}"#).await;
        let (iam, iam_seen) = mock(
            200,
            r#"{"accessToken": "impersonated", "expireTime": "2099-01-01T00:00:00Z"}"#,
        )
        .await;
        let federation = federation(Endpoints {
            aws_sts: None,
            google_sts: sts,
            google_iam: iam,
        });
        let egress = Egress::unguarded(reqwest::Client::new());
        let pool = Federated::GoogleCloud {
            provider: "projects/123/locations/global/workloadIdentityPools/p/providers/r".into(),
            project_id: "acme-ml".into(),
            service_account: Some("judge@acme-ml.iam.gserviceaccount.com".into()),
        };
        let live = federation
            .sign_in(
                &egress,
                &OrganizationId::new(),
                &ProjectId::new(),
                Uuid::new_v4(),
                &pool,
                None,
            )
            .await
            .expect("signed in");
        assert!(
            matches!(&live, Credentials::GoogleToken { token, project_id } if token.expose() == "impersonated" && project_id == "acme-ml")
        );
        let exchange: Value = serde_json::from_str(&sts_seen.lock().unwrap()[0]).unwrap();
        assert_eq!(
            exchange["audience"],
            "//iam.googleapis.com/projects/123/locations/global/workloadIdentityPools/p/providers/r"
        );
        assert_eq!(iam_seen.lock().unwrap().len(), 1);
    }
}
