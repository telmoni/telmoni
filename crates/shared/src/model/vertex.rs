//! Vertex AI's access token, from a service account's JSON key: an RS256 JWT
//! signed with its private key, exchanged at Google's token endpoint.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use super::Refused;
use crate::Redacted;
use crate::net_guard::Egress;

/// Google's token endpoint. The key's own `token_uri` is never followed, so
/// a key names no host the token is sent to.
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// What the token may do: call Vertex AI, as Google's own clients ask.
const SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

/// The parts of a service account's JSON key a token takes.
#[derive(Clone)]
pub struct ServiceAccount {
    /// The account, which the assertion names as its issuer.
    pub client_email: String,
    /// Its Google Cloud project, where Vertex AI is called.
    pub project_id: String,
    /// The private key, PKCS#8 DER.
    key: std::sync::Arc<zeroize::Zeroizing<Vec<u8>>>,
}

#[derive(Deserialize)]
struct KeyFile {
    #[serde(rename = "type")]
    kind: String,
    client_email: String,
    project_id: String,
    private_key: String,
}

impl ServiceAccount {
    /// A service account's JSON key, as Google's console downloads it.
    ///
    /// # Errors
    ///
    /// JSON that is no service account's key, named without echoing it.
    pub fn read(json: &str) -> Result<Self, String> {
        let file: KeyFile = serde_json::from_str(json).map_err(|_| {
            "a service account's key is the JSON file Google Cloud gives".to_owned()
        })?;
        if file.kind != "service_account"
            || file.client_email.is_empty()
            || file.project_id.is_empty()
        {
            return Err("the JSON is no service account's key".into());
        }
        let body: String = file
            .private_key
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect();
        let der = STANDARD
            .decode(body.trim())
            .map_err(|_| "the key's private_key is no PEM".to_owned())?;
        ring::signature::RsaKeyPair::from_pkcs8(&der)
            .map_err(|_| "the key's private_key is no RSA key".to_owned())?;
        Ok(Self {
            client_email: file.client_email,
            project_id: file.project_id,
            key: std::sync::Arc::new(zeroize::Zeroizing::new(der)),
        })
    }

    /// The signed assertion the token endpoint takes, good for an hour from `now`.
    ///
    /// # Errors
    ///
    /// The key would not sign.
    pub fn assertion(&self, now: DateTime<Utc>) -> Result<String, Refused> {
        let pair = ring::signature::RsaKeyPair::from_pkcs8(&self.key)
            .map_err(|_| Refused::Unreadable("the service account's key is no RSA key".into()))?;
        let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "RS256", "typ": "JWT" }).to_string());
        let claims = URL_SAFE_NO_PAD.encode(
            json!({
                "iss": self.client_email,
                "scope": SCOPE,
                "aud": TOKEN_URL,
                "iat": now.timestamp(),
                "exp": now.timestamp() + 3600,
            })
            .to_string(),
        );
        let message = format!("{header}.{claims}");
        let mut signature = vec![0; pair.public().modulus_len()];
        pair.sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &ring::rand::SystemRandom::new(),
            message.as_bytes(),
            &mut signature,
        )
        .map_err(|_| Refused::Unreadable("the service account's key would not sign".into()))?;
        Ok(format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature)))
    }
}

/// A token for `account`, exchanged now.
///
/// # Errors
///
/// Google refused the key, or did not answer.
pub async fn token(
    egress: &Egress,
    account: &ServiceAccount,
    now: DateTime<Utc>,
) -> Result<Redacted, Refused> {
    let body = format!(
        "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Ajwt-bearer&assertion={}",
        account.assertion(now)?
    );
    let response = egress
        .post(TOKEN_URL)
        .map_err(|rejected| Refused::Unreached(rejected.reason().into()))?
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|_| Refused::Unreached("Google's token endpoint did not answer".into()))?;
    let status = response.status().as_u16();
    let answer: Value = response
        .json()
        .await
        .map_err(|_| Refused::Unreadable("Google's token answer is not JSON".into()))?;
    match status {
        200..=299 => answer
            .get("access_token")
            .and_then(Value::as_str)
            .map(|t| Redacted::from(t.to_owned()))
            .ok_or_else(|| Refused::Unreadable("Google answered no token".into())),
        400 | 401 | 403 => Err(Refused::Unauthorized {
            status,
            detail: answer
                .get("error_description")
                .and_then(Value::as_str)
                .unwrap_or("the service account's key was refused")
                .to_owned(),
        }),
        _ => Err(Refused::Provider {
            status,
            detail: "Google's token endpoint failed".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    /// The suites' RSA key, as Google's console writes a service account's.
    fn key_file() -> String {
        let b64 = include_str!("../test_util/fixtures/id_token_test_key.pkcs8.b64").trim();
        json!({
            "type": "service_account",
            "project_id": "acme-ai",
            "client_email": "judge@acme-ai.iam.gserviceaccount.com",
            "private_key": format!("-----BEGIN PRIVATE KEY-----\n{b64}\n-----END PRIVATE KEY-----\n"),
            "token_uri": "https://attacker.example/token",
        })
        .to_string()
    }

    /// A key reads to its account, and its assertion is an RS256 JWT for
    /// Google's endpoint, signed so its public key verifies it.
    #[test]
    fn a_service_account_signs_an_assertion_for_googles_endpoint() {
        let account = ServiceAccount::read(&key_file()).expect("read");
        assert_eq!(account.project_id, "acme-ai");
        let at = Utc
            .with_ymd_and_hms(2026, 11, 2, 12, 0, 0)
            .single()
            .expect("a time");
        let jwt = account.assertion(at).expect("signed");
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).expect("b64")).expect("json");
        assert_eq!(claims["aud"], TOKEN_URL, "never the key's own token_uri");
        assert_eq!(claims["exp"].as_i64(), Some(at.timestamp() + 3600));
        let pair = ring::signature::RsaKeyPair::from_pkcs8(&account.key).expect("a key");
        let public = ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            pair.public().as_ref().to_vec(),
        );
        public
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &URL_SAFE_NO_PAD.decode(parts[2]).expect("b64"),
            )
            .expect("its public key verifies it");
    }

    /// JSON that is no service account's key is refused, and says nothing of what it held.
    #[test]
    fn json_that_is_no_service_account_is_refused() {
        assert!(ServiceAccount::read("{}").is_err());
        assert!(
            ServiceAccount::read(&key_file().replace("service_account", "authorized_user"))
                .is_err()
        );
        let refused = ServiceAccount::read(r#"{"type":"service_account","client_email":"a","project_id":"p","private_key":"secret-ish"}"#)
            .err()
            .expect("refused");
        assert!(!refused.contains("secret-ish"));
    }
}
