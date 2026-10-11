//! How an export reaches the platform: the one trait, its answer, and the
//! transport a client is built on unless [`Hooks`](crate::Hooks) gives another.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

/// Sends one export and gives back the answer, whatever its status; `Err`
/// only when no answer came back at all.
pub trait Transport: Send + Sync {
    /// `POST body` to `url` with `headers`, each a `(name, value)` pair.
    fn send(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Answer, TransportError>;
}

/// What the platform answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The headers, as `(name, value)` pairs; names match whatever their case.
    pub headers: Vec<(String, String)>,
    /// The body, as sent.
    pub body: Vec<u8>,
}

impl Answer {
    /// The first header named `name`, whatever its case.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// No answer came back: the platform could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportError {
    /// The error's name, which the failure warning carries, and nothing more:
    /// a message could quote the request.
    pub name: String,
}

impl TransportError {
    /// An error named `name`.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}

impl std::error::Error for TransportError {}

/// How long one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether `host`, as `Url::host_str` spells it, is this machine: `localhost`,
/// a name under it, or a loopback address. An IPv6 address comes in brackets.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    match bare.parse::<std::net::IpAddr>() {
        Ok(address) => address.is_loopback(),
        Err(_) => host == "localhost" || host.ends_with(".localhost"),
    }
}

/// The host a plain-HTTP `endpoint` would send the key to in the clear, when
/// that host is not this machine; `None` when the endpoint is fine to use.
pub(crate) fn cleartext_host(endpoint: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(endpoint).ok()?;
    if parsed.scheme() != "http" {
        return None;
    }
    let host = parsed.host_str().unwrap_or_default();
    (!is_loopback_host(host)).then(|| host.to_owned())
}

/// reqwest's blocking client, built at the first send. That send comes from
/// the processor's own thread, which matters: the blocking client refuses to
/// be built or used inside an async runtime, and an application may well build
/// its [`Telmoni`](crate::Telmoni) inside one.
#[derive(Default)]
pub(crate) struct Reqwest {
    client: Mutex<Option<reqwest::blocking::Client>>,
}

impl Reqwest {
    fn client(&self) -> Result<reqwest::blocking::Client, TransportError> {
        let mut slot = crate::lock(&self.client);
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        // No redirect is followed: followed, one would resend the bearer to
        // wherever it points; the 3xx comes back as the answer it is.
        let client = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| TransportError::new(kind(&error)))?;
        *slot = Some(client.clone());
        Ok(client)
    }
}

impl Transport for Reqwest {
    fn send(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Answer, TransportError> {
        let mut request = self.client()?.post(url).body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let response = request
            .send()
            .map_err(|error| TransportError::new(kind(&error)))?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_owned(), value.to_owned()))
            })
            .collect();
        let body = response
            .bytes()
            .map_err(|error| TransportError::new(kind(&error)))?
            .to_vec();
        Ok(Answer {
            status,
            headers,
            body,
        })
    }
}

/// The error's kind, as reqwest tells them apart: the name the warning
/// carries, since the message may quote the URL.
fn kind(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "Timeout"
    } else if error.is_connect() {
        "Connect"
    } else if error.is_builder() {
        "Builder"
    } else if error.is_redirect() {
        "Redirect"
    } else if error.is_body() {
        "Body"
    } else if error.is_decode() {
        "Decode"
    } else if error.is_request() {
        "Request"
    } else {
        "Transport"
    }
}
