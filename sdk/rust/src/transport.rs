//! How an export reaches the platform: the one trait, its answer, and the
//! transport a client is built on unless [`Hooks`](crate::Hooks) gives another.

use std::fmt;
use std::io::Read;
use std::net::{Ipv4Addr, Ipv6Addr};
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

/// How much of an answer is read: a problem's JSON is small, and an export
/// never pays for more.
const ANSWER_MAX: u64 = 1 << 20;

/// What a host's name is written in.
const NAME: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-";

/// What a path is written in: nothing any SDK's request escapes or rewrites.
const PATH: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789._-~!$&'()*+,;=:@/";

/// The host `endpoint` names, as written, or `None` where it is no URL every SDK reads
/// alike (`sdk/fixtures/endpoints.json`): nothing a URL parser fills in or rewrites.
pub(crate) fn host_of(endpoint: &str) -> Option<&str> {
    if !endpoint.bytes().all(|byte| (b'!'..=b'~').contains(&byte)) {
        return None;
    }
    let (scheme, rest) = endpoint.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if !only(path, PATH)
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let (host, port) = authority.split_at(authority.find(']')? + 1);
        let inner = host.strip_prefix('[')?.strip_suffix(']')?;
        let digits = inner
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b':' || byte == b'.');
        if !digits || inner.parse::<Ipv6Addr>().is_err() {
            return None;
        }
        (host, port)
    } else {
        let (host, port) = authority
            .find(':')
            .map_or((authority, ""), |at| authority.split_at(at));
        // WHATWG's parser reads a name ending in a number as an IPv4 address.
        let address = host.parse::<Ipv4Addr>().is_ok();
        let labels = host.strip_suffix('.').unwrap_or(host).split('.');
        if host.is_empty()
            || !only(host, NAME)
            || !labels.into_iter().all(label_ok)
            || (numbered(host) && !address)
        {
            return None;
        }
        (host, port)
    };
    port_ok(port).then_some(host)
}

/// A name's label as DNS and TLS take it: 1 to 63 characters, no hyphen at
/// either end.
fn label_ok(label: &str) -> bool {
    (1..=63).contains(&label.len()) && !label.starts_with('-') && !label.ends_with('-')
}

/// Whether every byte of `text` is one of `set`'s.
fn only(text: &str, set: &str) -> bool {
    text.bytes().all(|byte| set.as_bytes().contains(&byte))
}

/// Whether `host`'s last label, a trailing dot aside, is a number.
fn numbered(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    let last = host.rsplit('.').next().unwrap_or_default();
    let decimal = !last.is_empty() && last.bytes().all(|byte| byte.is_ascii_digit());
    let hexadecimal = last
        .get(..2)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("0x"))
        && last.bytes().skip(2).all(|byte| byte.is_ascii_hexdigit());
    decimal || hexadecimal
}

/// Whether `port` is none, an empty one, or up to five digits from 1 to 65535.
fn port_ok(port: &str) -> bool {
    match port.strip_prefix(':') {
        None => port.is_empty(),
        Some(digits) => {
            digits.is_empty()
                || (digits.len() <= 5
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                    && digits
                        .parse::<u32>()
                        .is_ok_and(|n| (1..=65_535).contains(&n)))
        }
    }
}

/// Whether `host`, as [`host_of`] gives it, is this machine: `localhost`, a
/// name under it, 127.0.0.0/8 or ::1, an IPv4-mapped address read as its IPv4.
pub(crate) fn is_this_machine(host: &str) -> bool {
    if let Some(inner) = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
    {
        return inner.parse::<Ipv6Addr>().is_ok_and(|address| {
            address.is_loopback() || address.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        });
    }
    if let Ok(address) = host.parse::<Ipv4Addr>() {
        return address.is_loopback();
    }
    let name = host.to_ascii_lowercase();
    name == "localhost" || name.ends_with(".localhost")
}

/// Why nothing goes to `endpoint`: one the SDKs could read two ways fails an export or
/// reaches another host, and plain HTTP to another machine carries the key in the clear.
pub(crate) fn refusal(endpoint: &str) -> Option<String> {
    let Some(host) = host_of(endpoint) else {
        return Some("the endpoint must be an http:// or https:// URL".to_owned());
    };
    let http = endpoint
        .split_once("://")
        .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("http"));
    (http && !is_this_machine(host)).then(|| {
        format!("refusing to send the API key over unencrypted HTTP to '{host}'; use HTTPS")
    })
}

/// Whether `url` goes straight to this machine, past any proxy the environment
/// names: reqwest exempts no loopback host from them, and a proxy would take plain
/// HTTP's key off this machine in the clear; one elsewhere cannot reach its ports.
pub(crate) fn bypasses_proxy(url: &str) -> bool {
    host_of(url).is_some_and(is_this_machine)
}

/// reqwest's blocking clients, each built at its first send. That send comes from
/// the processor's own thread, which matters: the blocking client refuses to
/// be built or used inside an async runtime, and an application may well build
/// its [`Telmoni`](crate::Telmoni) inside one.
#[derive(Default)]
pub(crate) struct Reqwest {
    /// Every other host, through any proxy the environment names.
    client: Mutex<Option<reqwest::blocking::Client>>,
    /// This machine, through none ([`bypasses_proxy`]).
    direct: Mutex<Option<reqwest::blocking::Client>>,
}

impl Reqwest {
    fn client(&self, url: &str) -> Result<reqwest::blocking::Client, TransportError> {
        let direct = bypasses_proxy(url);
        let mut slot = crate::lock(if direct { &self.direct } else { &self.client });
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        // No redirect is followed: followed, one would resend the bearer to
        // wherever it points; the 3xx comes back as the answer it is. Nor is
        // an answer decoded, whatever features the application turns on.
        let mut builder = reqwest::blocking::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd();
        if direct {
            builder = builder.no_proxy();
        }
        let client = builder
            .build()
            .map_err(|error| TransportError::new(kind(&error)))?;
        *slot = Some(client.clone());
        Ok(client)
    }
}

impl Reqwest {
    /// Sends one export, its answer's head and body held to `within` of its start.
    fn post(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
        within: Duration,
    ) -> Result<Answer, TransportError> {
        // The request's own timeout ends a read under way; the client's starts
        // again at each read, so a body that stalls would outlast it.
        let mut request = self
            .client(url)?
            .post(url)
            .timeout(within)
            .body(body.to_vec());
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let response = request
            .send()
            .map_err(|error| TransportError::new(kind(&error)))?;
        Ok(answer(response))
    }
}

impl Transport for Reqwest {
    fn send(
        &self,
        url: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Result<Answer, TransportError> {
        self.post(url, headers, body, REQUEST_TIMEOUT)
    }
}

/// What `response` answered, at most [`ANSWER_MAX`] bytes of its body read
/// before the request's deadline. Its status decides: a body that breaks after
/// it, or outlasts the deadline, is kept as far as it came, as in every SDK.
fn answer(response: reqwest::blocking::Response) -> Answer {
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
    let mut body = Vec::new();
    // `read_to_end` keeps what it read before an error.
    let _ = response.take(ANSWER_MAX).read_to_end(&mut body);
    Answer {
        status,
        headers,
        body,
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

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// One request's head and body read off `stream`: its request line, and its
    /// `Authorization`, or nothing.
    fn request(stream: TcpStream) -> (String, String, TcpStream) {
        let mut reader = BufReader::new(stream);
        let mut first = String::new();
        reader.read_line(&mut first).unwrap();
        let (mut length, mut key) = (0, String::new());
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" || line.is_empty() {
                break;
            }
            let lower = line.to_ascii_lowercase();
            if let Some(value) = lower.strip_prefix("content-length:") {
                length = value.trim().parse().unwrap();
            }
            if lower.starts_with("authorization:")
                && let Some((_, value)) = line.split_once(':')
            {
                key = value.trim().to_owned();
            }
        }
        reader.read_exact(&mut vec![0; length]).unwrap();
        (first.trim_end().to_owned(), key, reader.into_inner())
    }

    /// Each request's line and key, as a server was sent them.
    type Seen = Arc<Mutex<Vec<(String, String)>>>;

    /// A loopback server's port that answers every request `{}`, and what it
    /// was sent.
    fn recording() -> (u16, Seen) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (line, key, mut stream) = request(stream);
                crate::lock(&kept).push((line, key));
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        (port, seen)
    }

    #[test]
    fn this_machine_is_reached_through_no_proxy() {
        for url in [
            "http://localhost:7100/otlp/v1/traces",
            "http://LOCALHOST:7100/otlp/v1/traces",
            "http://api.localhost:7100/otlp/v1/traces",
            "http://127.0.0.2:7100/otlp/v1/traces",
            "http://[::1]:7100/otlp/v1/traces",
            "http://[::ffff:127.0.0.1]:7100/otlp/v1/traces",
            "https://localhost/otlp/v1/traces",
        ] {
            assert!(bypasses_proxy(url), "{url}");
        }
        assert!(!bypasses_proxy("https://api.telmoni.com/otlp/v1/traces"));
    }

    /// A proxy the environment names, which reqwest takes for this machine too,
    /// never stands between an export and this machine: it would read the key.
    #[test]
    fn a_proxy_the_environment_names_is_passed_for_this_machine() {
        let (proxy, taken) = recording();
        let (port, sent) = recording();
        let named = format!("http://127.0.0.1:{proxy}");
        // A process of its own, which reads the environment afresh.
        let helper = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transport::tests::proxy_environment_helper"])
            .args(["--ignored", "--quiet"])
            .env("TELMONI_PROXY_HELPER", port.to_string())
            .env("HTTP_PROXY", &named)
            .env("http_proxy", &named)
            .env("ALL_PROXY", &named)
            .env_remove("NO_PROXY")
            .env_remove("no_proxy")
            .output()
            .unwrap();
        assert!(
            helper.status.success(),
            "{}",
            String::from_utf8_lossy(&helper.stdout)
        );
        let control = format!("POST http://127.0.0.1:{port}/control HTTP/1.1");
        assert_eq!(*crate::lock(&taken), vec![(control, String::new())]);
        let line = "POST /otlp/v1/traces HTTP/1.1".to_owned();
        assert_eq!(*crate::lock(&sent), vec![(line, "Bearer k".to_owned())]);
    }

    /// The process [`a_proxy_the_environment_names_is_passed_for_this_machine`] starts.
    #[test]
    #[ignore = "started by a_proxy_the_environment_names_is_passed_for_this_machine"]
    fn proxy_environment_helper() {
        let Ok(port) = std::env::var("TELMONI_PROXY_HELPER") else {
            return;
        };
        // reqwest's own client takes the environment's proxy, for this machine too.
        let _ = reqwest::blocking::Client::new()
            .post(format!("http://127.0.0.1:{port}/control"))
            .body("{}")
            .send();
        let key = [("Authorization".to_owned(), "Bearer k".to_owned())];
        let url = format!("http://LOCALHOST:{port}/otlp/v1/traces");
        let answer = Reqwest::default().send(&url, &key, b"{}").unwrap();
        assert_eq!(answer.status, 200);
    }

    /// As in every SDK: a status line alone, its head cut before the blank line
    /// that ends it, decides nothing.
    #[test]
    fn a_head_cut_before_its_blank_line_is_no_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/otlp/v1/traces", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let (_, _, mut stream) = request(stream);
            let _ = stream.write_all(b"HTTP/1.1 413 Payload Too Large\r\nContent-Ty");
        });
        assert!(Reqwest::default().send(&url, &[], b"{}").is_err());
    }

    /// The answer past every 1xx before it, as in every SDK, and an upgrade nobody
    /// asked for a refusal, as in Python and Go.
    #[test]
    fn the_answer_is_read_past_every_1xx_before_it_but_a_101() {
        for (reply, status, mode) in [
            (
                "HTTP/1.1 103 Early Hints\r\nLink: </x>\r\n\r\nHTTP/1.1 102 Processing\r\n\r\n\
                 HTTP/1.1 200 OK\r\nTelmoni-Content-Mode: on\r\nContent-Length: 2\r\n\r\n{}",
                200,
                Some("on"),
            ),
            (
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: x\r\nConnection: Upgrade\r\n\r\n",
                101,
                None,
            ),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}/otlp/v1/traces", listener.local_addr().unwrap());
            thread::spawn(move || {
                let (stream, _) = listener.accept().unwrap();
                let (_, _, mut stream) = request(stream);
                let _ = stream.write_all(reply.as_bytes());
            });
            let answer = Reqwest::default().send(&url, &[], b"{}").unwrap();
            assert_eq!(
                (answer.status, answer.header("Telmoni-Content-Mode")),
                (status, mode)
            );
        }
    }

    /// A loopback server's URL that answers one export with `status`, `size` bytes declared
    /// and `sent` sent, one every `gap` unless it is zero, before it hangs up.
    fn answering(status: u16, size: usize, sent: usize, gap: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/otlp/v1/traces", listener.local_addr().unwrap());
        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            reader.read_exact(&mut vec![0; length]).unwrap();
            let mut stream = reader.into_inner();
            // The client may hang up before the body's end: that is the point.
            let _ = write!(
                stream,
                "HTTP/1.1 {status} Answer\r\nContent-Length: {size}\r\n\r\n"
            );
            if gap.is_zero() {
                let _ = stream.write_all(&vec![b'x'; sent]);
                return;
            }
            for _ in 0..sent {
                thread::sleep(gap);
                if stream.write_all(b"x").is_err() {
                    return;
                }
            }
        });
        url
    }

    #[test]
    fn an_answer_is_read_up_to_its_first_mebibyte() {
        let transport = Reqwest::default();
        for (status, size, read) in [
            (200, 2 << 20, 1 << 20),
            (503, 2 << 20, 1 << 20),
            (200, 300, 300),
        ] {
            let url = answering(status, size, size, Duration::ZERO);
            let answer = transport.send(&url, &[], b"{}").unwrap();
            assert_eq!((answer.status, answer.body.len()), (status, read));
        }
    }

    /// The status decides: a body that breaks after it is no failure to retry.
    #[test]
    fn an_answer_whose_body_breaks_is_kept_as_far_as_it_came() {
        let transport = Reqwest::default();
        for status in [200, 413] {
            let url = answering(status, 300, 100, Duration::ZERO);
            let answer = transport.send(&url, &[], b"{}").unwrap();
            assert_eq!((answer.status, answer.body), (status, vec![b'x'; 100]));
        }
    }

    #[test]
    fn an_answer_sent_slowly_is_given_up_at_the_deadline_as_far_as_it_came() {
        let url = answering(200, 100, 100, Duration::from_millis(50));
        let began = Instant::now();
        let answer = Reqwest::default()
            .post(&url, &[], b"{}", Duration::from_millis(300))
            .unwrap();
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
        assert_eq!(answer.status, 200);
        assert!(
            answer.body.len() < 100 && answer.body.iter().all(|byte| *byte == b'x'),
            "{}",
            answer.body.len()
        );
    }

    /// A body that stalls ends at the deadline, not at a read's own wait past it.
    #[test]
    fn an_answer_whose_body_stalls_is_given_up_at_the_deadline() {
        let url = answering(200, 100, 2, Duration::from_secs(3));
        let began = Instant::now();
        let answer = Reqwest::default()
            .post(&url, &[], b"{}", Duration::from_millis(300))
            .unwrap();
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "{:?}",
            began.elapsed()
        );
        assert_eq!((answer.status, answer.body.len()), (200, 0));
    }
}
