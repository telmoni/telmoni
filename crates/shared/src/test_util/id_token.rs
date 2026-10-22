//! A stand-in identity provider's signing key: an RSA-2048 fixture that
//! signs id tokens the way an OpenID Connect provider does, so a suite can
//! exercise the verifier and the code exchange with no provider.
//!
//! ⚠ The private half is committed. It is test material and nothing else:
//! nothing deployed trusts it, because a verifier takes keys only from the
//! key set its provider publishes, and a suite serves this one from a mock
//! endpoint.

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

/// PKCS#8 DER, base64.
const KEY_PKCS8_B64: &str = include_str!("fixtures/id_token_test_key.pkcs8.b64");

/// The `client_id` a suite's verifier is built for, and its id tokens name
/// in `aud`.
pub const TEST_CLIENT_ID: &str = "client_test_123";

/// The issuer a suite's provider stands at: what its id tokens carry in
/// `iss`. `.invalid` is reserved and never resolves.
pub const TEST_PROVIDER_ISSUER: &str = "https://idp.telmoni.invalid";

/// The `kid` the stand-in provider publishes its key under.
pub const PROVIDER_KID: &str = "telmoni-test-provider";

fn key_pair() -> ring::signature::RsaKeyPair {
    let der = STANDARD
        .decode(KEY_PKCS8_B64.trim())
        .expect("the fixture is base64");
    ring::signature::RsaKeyPair::from_pkcs8(&der).expect("the fixture is a PKCS#8 RSA key")
}

/// The provider's key set, as its `jwks_uri` would serve it.
#[must_use]
pub fn provider_jwks_json() -> String {
    let components: ring::signature::RsaPublicKeyComponents<Vec<u8>> = key_pair().public().into();
    serde_json::json!({
        "keys": [{
            "kty": "RSA",
            "kid": PROVIDER_KID,
            "alg": "RS256",
            "use": "sig",
            "n": URL_SAFE_NO_PAD.encode(components.n),
            "e": URL_SAFE_NO_PAD.encode(components.e),
        }]
    })
    .to_string()
}

/// An id token over exactly these claims, signed as the provider signs one.
#[must_use]
pub fn provider_id_token(claims: serde_json::Value) -> String {
    id_token_under(PROVIDER_KID, claims)
}

/// An id token over exactly these claims, signed by the provider's key but
/// naming `kid` in its header.
#[must_use]
pub fn id_token_under(kid: &str, claims: serde_json::Value) -> String {
    let header = serde_json::json!({ "alg": "RS256", "kid": kid, "typ": "JWT" });
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header serialises")),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims serialise")),
    );
    let key_pair = key_pair();
    let mut signature = vec![0; key_pair.public().modulus_len()];
    key_pair
        .sign(
            &ring::signature::RSA_PKCS1_SHA256,
            &ring::rand::SystemRandom::new(),
            signing_input.as_bytes(),
            &mut signature,
        )
        .expect("the fixture signs");
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

/// An id token over exactly these claims, signed by the provider's RSA key
/// with RSASSA-PSS over SHA-256, `PS256` in its header.
#[must_use]
pub fn ps256_id_token(claims: serde_json::Value) -> String {
    let key_pair = key_pair();
    sign_as("PS256", PROVIDER_KID, &claims, |input| {
        let mut signature = vec![0; key_pair.public().modulus_len()];
        key_pair
            .sign(
                &ring::signature::RSA_PSS_SHA256,
                &ring::rand::SystemRandom::new(),
                input,
                &mut signature,
            )
            .expect("the fixture signs");
        signature
    })
}

/// The provider's RSA key published with no `alg`, as a provider that signs
/// with several does.
#[must_use]
pub fn provider_jwks_json_any_alg() -> String {
    let mut set: serde_json::Value =
        serde_json::from_str(&provider_jwks_json()).expect("the fixture's set");
    if let Some(key) = set
        .get_mut("keys")
        .and_then(|keys| keys.get_mut(0))
        .and_then(serde_json::Value::as_object_mut)
    {
        key.remove("alg");
    }
    set.to_string()
}

/// A provider that signs with an EC key on P-256, made for one suite.
pub struct EcProvider {
    key_pair: ring::signature::EcdsaKeyPair,
}

impl EcProvider {
    /// A fresh P-256 key.
    #[must_use]
    pub fn p256() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let der = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).expect("a key");
        Self {
            key_pair: ring::signature::EcdsaKeyPair::from_pkcs8(alg, der.as_ref(), &rng)
                .expect("the key reads back"),
        }
    }

    /// Its key set, the key under [`PROVIDER_KID`].
    #[must_use]
    pub fn jwks_json(&self) -> String {
        use ring::signature::KeyPair as _;
        // The uncompressed point: 0x04, then x, then y, 32 bytes each.
        let point = self.key_pair.public_key().as_ref();
        let (x, y) = (
            point.get(1..33).expect("a P-256 point's x"),
            point.get(33..65).expect("a P-256 point's y"),
        );
        serde_json::json!({
            "keys": [{
                "kty": "EC",
                "crv": "P-256",
                "kid": PROVIDER_KID,
                "use": "sig",
                "x": URL_SAFE_NO_PAD.encode(x),
                "y": URL_SAFE_NO_PAD.encode(y),
            }]
        })
        .to_string()
    }

    /// An id token over exactly these claims, `ES256` in its header.
    #[must_use]
    pub fn id_token(&self, claims: serde_json::Value) -> String {
        sign_as("ES256", PROVIDER_KID, &claims, |input| {
            self.key_pair
                .sign(&ring::rand::SystemRandom::new(), input)
                .expect("the key signs")
                .as_ref()
                .to_vec()
        })
    }
}

/// A token's signing input under a header naming `alg` and `kid`, and the
/// signature `sign` makes of it.
fn sign_as(
    alg: &str,
    kid: &str,
    claims: &serde_json::Value,
    sign: impl FnOnce(&[u8]) -> Vec<u8>,
) -> String {
    let header = serde_json::json!({ "alg": alg, "kid": kid, "typ": "JWT" });
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("header serialises")),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims serialise")),
    );
    let signature = sign(signing_input.as_bytes());
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}
