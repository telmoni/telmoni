//! A model asked once for a judgement in a schema, on a connection the
//! customer made with their own key: what an evaluator sends. Each vendor's
//! wire is its own module, a request built and an answer read as pure
//! functions; [`judge`] sends one through [`Egress`], since the customer
//! names the host, and follows no redirect.

pub mod anthropic;
pub mod bedrock;
pub mod gemini;
pub mod openai;
pub mod sigv4;
pub mod vertex;

use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::net_guard::{Egress, HostRejection};
use crate::{ModelProvider, Redacted};

/// The name the answer's schema goes by, as a tool or a format.
pub const SCHEMA_NAME: &str = "score";

/// Who says a message to the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The person, or what a judged run was shown.
    User,
    /// The model, or what a judged run answered.
    Assistant,
}

/// One judgement asked for: the model, its instructions, the conversation it
/// is shown, and the JSON Schema its answer must meet.
#[derive(Debug, Clone)]
pub struct Ask<'a> {
    /// The model's name at its provider, or an Azure deployment's.
    pub model: &'a str,
    /// The instructions, as the system's message.
    pub system: &'a str,
    /// The conversation, in order.
    pub messages: &'a [(Role, String)],
    /// What the answer must be.
    pub schema: &'a Value,
    /// The provider's own default where none.
    pub temperature: Option<f64>,
    /// The most the answer may run to.
    pub max_tokens: u32,
}

/// The tokens a judgement used.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// What it was sent, as the provider counts it.
    pub input_tokens: u64,
    /// What it wrote.
    pub output_tokens: u64,
}

/// A judgement as the model gave it: its answer, read as JSON in the schema
/// asked for, and what it used.
#[derive(Debug, Clone, PartialEq)]
pub struct Judged {
    /// The answer, as JSON.
    pub answer: Value,
    /// What it used.
    pub usage: Usage,
}

/// Why a judgement came back without one.
#[derive(Debug, Clone, PartialEq)]
pub enum Refused {
    /// The key or the account was refused (401, 403): the connection's
    /// evaluators pause until it is fixed.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    Unauthorized { status: u16, detail: String },
    /// A rate limit: wait its `Retry-After`, where it gave one.
    #[expect(missing_docs, reason = "the variant says what its field is")]
    RateLimited { retry_after: Option<Duration> },
    /// Any other answer that was not a judgement, its status and what it said.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    Provider { status: u16, detail: String },
    /// An answer outside the schema, or no answer at all.
    Unreadable(String),
    /// No answer: the network, a timeout, or a host the guard refused.
    Unreached(String),
}

impl Refused {
    /// A line for the person whose connection it is, never the key.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::Unauthorized { status, detail } => {
                format!("the provider refused the key ({status}): {}", clip(detail))
            }
            Self::RateLimited { .. } => "the provider's rate limit holds; try again shortly".into(),
            Self::Provider { status, detail } => {
                format!("the provider answered {status}: {}", clip(detail))
            }
            Self::Unreadable(why) => format!("the model's answer could not be read: {why}"),
            Self::Unreached(why) => format!("the provider could not be reached: {why}"),
        }
    }
}

/// A provider's words, cut to a line, so no page of HTML reaches a toast.
fn clip(detail: &str) -> String {
    let line: String = detail
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect();
    if line.is_empty() {
        "no reason given".into()
    } else {
        line
    }
}

/// What a connection opens with: its secret, read from the sealed field, by
/// what its provider takes.
#[derive(Clone)]
pub enum Credentials {
    /// OpenAI, Anthropic, Google AI Studio, Azure OpenAI, or a compatible
    /// endpoint's key; empty for an endpoint that asks for none.
    ApiKey(Redacted),
    /// A Google Cloud service account's JSON key.
    ServiceAccount(vertex::ServiceAccount),
    /// An AWS access key pair.
    #[expect(missing_docs, reason = "the variant says what its fields are")]
    AccessKeys {
        access_key_id: String,
        secret_access_key: Redacted,
    },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ApiKey(_) => "ApiKey(..)",
            Self::ServiceAccount(_) => "ServiceAccount(..)",
            Self::AccessKeys { .. } => "AccessKeys(..)",
        })
    }
}

/// The AWS pair as a connection's secret holds it.
#[derive(Deserialize)]
struct AccessKeyPair {
    access_key_id: String,
    secret_access_key: String,
}

impl Credentials {
    /// `secret` as `provider` takes it: an API key as given, a service
    /// account's JSON key, or an access key pair as `{"access_key_id": …,
    /// "secret_access_key": …}`.
    ///
    /// # Errors
    ///
    /// A secret not in the shape its provider takes, named without echoing it.
    pub fn read(provider: ModelProvider, secret: &Redacted) -> Result<Self, String> {
        match provider {
            ModelProvider::VertexAi => {
                vertex::ServiceAccount::read(secret.expose()).map(Self::ServiceAccount)
            }
            ModelProvider::Bedrock => serde_json::from_str::<AccessKeyPair>(secret.expose())
                .map_err(|_| {
                    "an access key pair is JSON with access_key_id and secret_access_key".to_owned()
                })
                .and_then(|pair| {
                    if pair.access_key_id.trim().is_empty()
                        || pair.secret_access_key.trim().is_empty()
                    {
                        Err("an access key pair names both its id and its secret".to_owned())
                    } else {
                        Ok(Self::AccessKeys {
                            access_key_id: pair.access_key_id.trim().to_owned(),
                            secret_access_key: pair.secret_access_key.trim().to_owned().into(),
                        })
                    }
                }),
            ModelProvider::OpenaiCompatible => {
                Ok(Self::ApiKey(secret.expose().trim().to_owned().into()))
            }
            _ if secret.expose().trim().is_empty() => Err("a key is needed".to_owned()),
            _ => Ok(Self::ApiKey(secret.expose().trim().to_owned().into())),
        }
    }
}

/// A connection, opened: its provider, where it answers, and its secret.
#[derive(Debug, Clone)]
pub struct Connection {
    /// Whose model it reaches.
    pub provider: ModelProvider,
    /// The endpoint: a compatible one's base URL, an Azure resource's; none
    /// where the provider's own host is the only one.
    pub base_url: Option<String>,
    /// Bedrock's region, or Vertex AI's location.
    pub region: Option<String>,
    /// What it opens with.
    pub credentials: Credentials,
}

/// One request, built: where it goes, its headers, and its body.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Where it goes.
    pub url: String,
    /// Its headers, the credential among them, by lower-case name.
    pub headers: Vec<(String, Redacted)>,
    /// Its JSON body.
    pub body: Vec<u8>,
}

/// `value` set at `key` of a request's body, which every one here is an object.
fn set(body: &mut Value, key: &str, value: Value) {
    if let Some(object) = body.as_object_mut() {
        object.insert(key.to_owned(), value);
    }
}

/// Ask `connection`'s model `ask`, through `egress`.
///
/// # Errors
///
/// Why no judgement came back ([`Refused`]).
pub async fn judge(
    egress: &Egress,
    connection: &Connection,
    ask: &Ask<'_>,
) -> Result<Judged, Refused> {
    let prepared = match connection.provider {
        ModelProvider::Openai | ModelProvider::OpenaiCompatible | ModelProvider::AzureOpenai => {
            openai::request(connection, ask)?
        }
        ModelProvider::Anthropic => anthropic::request(connection, ask)?,
        ModelProvider::GoogleAiStudio => gemini::request(connection, ask, None)?,
        ModelProvider::VertexAi => {
            let Credentials::ServiceAccount(account) = &connection.credentials else {
                return Err(Refused::Unreadable(
                    "a Vertex AI connection holds a service account".into(),
                ));
            };
            let token = vertex::token(egress, account, chrono::Utc::now()).await?;
            gemini::request(connection, ask, Some(&token))?
        }
        ModelProvider::Bedrock => bedrock::request(connection, ask, chrono::Utc::now())?,
    };
    let (status, retry_after, body) = send(egress, &prepared).await?;
    let answered = |body: &Value| match connection.provider {
        ModelProvider::Openai | ModelProvider::OpenaiCompatible | ModelProvider::AzureOpenai => {
            openai::answer(body)
        }
        ModelProvider::Anthropic => anthropic::answer(body),
        ModelProvider::GoogleAiStudio | ModelProvider::VertexAi => gemini::answer(body),
        ModelProvider::Bedrock => bedrock::answer(body),
    };
    read(status, retry_after, &body, answered)
}

/// The answer of a request, sent: its status, its `Retry-After`, its body.
async fn send(
    egress: &Egress,
    prepared: &Prepared,
) -> Result<(u16, Option<Duration>, Vec<u8>), Refused> {
    let mut request = egress
        .post(&prepared.url)
        .map_err(|rejected: HostRejection| Refused::Unreached(rejected.reason().into()))?;
    for (name, value) in &prepared.headers {
        request = request.header(name.as_str(), value.expose());
    }
    let response = request
        .body(prepared.body.clone())
        .send()
        .await
        .map_err(|e| {
            Refused::Unreached(if e.is_timeout() {
                "it did not answer in time".into()
            } else {
                "no connection".into()
            })
        })?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    let body = response
        .bytes()
        .await
        .map_err(|_| Refused::Unreached("the answer was cut short".into()))?;
    Ok((status, retry_after, body.to_vec()))
}

/// An answer read by its status: a judgement where it is a 2xx the provider's
/// reader takes, else why not.
fn read(
    status: u16,
    retry_after: Option<Duration>,
    body: &[u8],
    answered: impl Fn(&Value) -> Result<Judged, Refused>,
) -> Result<Judged, Refused> {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let detail = || {
        parsed
            .as_ref()
            .and_then(|v| {
                v.pointer("/error/message")
                    .or_else(|| v.pointer("/message"))
                    .or_else(|| v.pointer("/error"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(body).chars().take(300).collect())
    };
    match status {
        200..=299 => answered(
            parsed
                .as_ref()
                .ok_or_else(|| Refused::Unreadable("its body is not JSON".into()))?,
        ),
        401 | 403 => Err(Refused::Unauthorized {
            status,
            detail: detail(),
        }),
        429 => Err(Refused::RateLimited { retry_after }),
        _ => Err(Refused::Provider {
            status,
            detail: detail(),
        }),
    }
}

/// `text` as the JSON the schema asked for: the whole of it, else the object
/// it holds between its first `{` and last `}`, as a model that wraps its
/// answer in a fence sends it.
fn json_in(text: &str) -> Result<Value, Refused> {
    serde_json::from_str(text.trim()).or_else(|_| {
        let (start, end) = (text.find('{'), text.rfind('}'));
        match (start, end) {
            (Some(start), Some(end)) if start < end => text
                .get(start..=end)
                .and_then(|inner| serde_json::from_str(inner).ok())
                .ok_or_else(|| Refused::Unreadable("its answer is not JSON".into())),
            _ => Err(Refused::Unreadable("its answer is not JSON".into())),
        }
    })
}

/// A count of tokens at `pointer` in `body`, none where it says none.
fn tokens(body: &Value, pointer: &str) -> u64 {
    body.pointer(pointer).and_then(Value::as_u64).unwrap_or(0)
}

/// `base` without a trailing `/`, `https` on a tier, where its key would
/// otherwise travel in the clear.
///
/// # Errors
///
/// A base URL that is no URL, or plain `http` on a deployed tier.
pub fn base_url(given: &str) -> Result<String, String> {
    let trimmed = given.trim().trim_end_matches('/');
    let parsed =
        reqwest::Url::parse(trimmed).map_err(|_| "the base URL is not a URL".to_owned())?;
    if !matches!(parsed.scheme(), "https" | "http") {
        return Err("the base URL is http or https".into());
    }
    if parsed.scheme() != "https" && crate::envelope::on_deployed_tier() {
        return Err("the base URL is https, so its key never travels in the clear".into());
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err("the base URL carries no user or password; the key does".into());
    }
    Ok(trimmed.to_owned())
}

/// Bedrock's region or Vertex AI's location: letters, digits and hyphens.
///
/// # Errors
///
/// One that is no region's name.
pub fn region(given: &str) -> Result<String, String> {
    let trimmed = given.trim();
    if trimmed.is_empty()
        || trimmed.len() > 32
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err("a region is its name, such as us-east-1 or us-central1".into());
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer is read by its status: a refusal of the key apart from a
    /// rate limit and every other failure, and a fenced JSON answer read.
    #[test]
    fn an_answer_is_read_by_its_status() {
        let never = |_: &Value| -> Result<Judged, Refused> { unreachable!() };
        assert!(matches!(
            read(401, None, br#"{"error":{"message":"bad key"}}"#, never),
            Err(Refused::Unauthorized { status: 401, detail }) if detail == "bad key"
        ));
        assert_eq!(
            read(429, Some(Duration::from_secs(7)), b"", never),
            Err(Refused::RateLimited {
                retry_after: Some(Duration::from_secs(7))
            })
        );
        assert!(matches!(
            read(500, None, b"<html>", never),
            Err(Refused::Provider { status: 500, .. })
        ));
        assert_eq!(
            json_in("```json\n{\"score\": 1}\n```").expect("read"),
            serde_json::json!({ "score": 1 })
        );
        assert!(json_in("no").is_err());
    }

    /// A base URL is https on a tier and carries no credential; a region is a name.
    #[test]
    fn a_base_url_and_a_region_are_held_to_their_shape() {
        assert_eq!(
            base_url("https://gw.example/v1/").expect("kept"),
            "https://gw.example/v1"
        );
        assert!(base_url("ftp://x").is_err());
        assert!(base_url("https://u:p@gw.example").is_err());
        assert_eq!(region(" us-east-1 ").expect("kept"), "us-east-1");
        assert!(region("us east").is_err());
    }

    /// A secret is read as its provider takes it, a pair as JSON.
    #[test]
    fn a_secret_is_read_as_its_provider_takes_it() {
        assert!(Credentials::read(ModelProvider::Openai, &"  ".to_owned().into()).is_err());
        assert!(Credentials::read(ModelProvider::OpenaiCompatible, &String::new().into()).is_ok());
        let pair = r#"{"access_key_id":"AKID","secret_access_key":"s3cret"}"#.to_owned();
        assert!(matches!(
            Credentials::read(ModelProvider::Bedrock, &pair.into()),
            Ok(Credentials::AccessKeys { access_key_id, .. }) if access_key_id == "AKID"
        ));
        assert!(Credentials::read(ModelProvider::Bedrock, &"AKID:s".to_owned().into()).is_err());
    }
}
