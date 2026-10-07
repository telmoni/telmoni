//! SMTP as the [`MailSender`]: the one transport every mail service accepts,
//! so a deployment names its server in `SMTP_URL` and nothing here knows
//! whose it is.
//!
//! `SMTP_URL` is a connection URL, and the scheme decides the encryption:
//! - `smtps://user:password@host:465` — TLS from the first byte;
//! - `smtp://user:password@host:587?tls=required` — STARTTLS, and a server
//!   that does not offer it is refused;
//! - `smtp://host:1025` — plaintext, for a relay on the same machine or a
//!   development catcher.
//!
//! The user name and password are percent-encoded, as in any URL. The URL
//! carries a credential, so it is held as [`Redacted`] and never logged,
//! whole or in part.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::header::{ContentType, HeaderName, HeaderValue};
use lettre::message::{Mailbox, MultiPart};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use telmoni_shared::Redacted;
use telmoni_shared::config::optional;
use telmoni_shared::envelope::on_deployed_tier;
use telmoni_shared::mail::{Mail, MailError, MailSender, NoopSender};

use crate::Config;

/// How long one step of the conversation may take. The lanes that mail send
/// inline, and the console gives up on them at ten seconds.
const STEP_TIMEOUT: Duration = Duration::from_secs(5);

const LIST_UNSUBSCRIBE: HeaderName = HeaderName::new_from_ascii_str("List-Unsubscribe");
const LIST_UNSUBSCRIBE_POST: HeaderName = HeaderName::new_from_ascii_str("List-Unsubscribe-Post");

/// Mail over SMTP, with a pool of connections kept open between sends.
pub struct SmtpSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpSender {
    /// The transport `url` names, sending as `from` unless a mail names its
    /// own. Nothing connects until the first send.
    pub fn new(url: &Redacted, from: &str) -> anyhow::Result<Self> {
        let from: Mailbox = from
            .parse()
            .map_err(|_| anyhow::anyhow!("MAIL_FROM is not an address mail can be sent as"))?;
        // The parser's error is dropped rather than printed: the URL is the
        // credential, and what the fix needs is the shape it should have.
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url.expose())
            .map_err(|_| {
                anyhow::anyhow!(
                    "SMTP_URL is not a connection URL: smtps://user:password@host:465, \
                     smtp://user:password@host:587?tls=required, or smtp://host:port"
                )
            })?
            .timeout(Some(STEP_TIMEOUT))
            .build();
        Ok(Self { transport, from })
    }
}

/// The transport `SMTP_URL` names, or the log when it is unset. One function
/// decides what "unset" means and says so at startup, for every binary built
/// on this library: a service that boots green and delivers nothing must be
/// visible.
pub fn transport_from_env(config: &Config) -> anyhow::Result<Arc<dyn MailSender>> {
    let Some(url) = optional("SMTP_URL") else {
        tracing::warn!(
            "outbound mail: SMTP_URL is unset — nothing is sent; each mail leaves one log line, without its contents"
        );
        return Ok(Arc::new(NoopSender));
    };
    let url = Redacted::from(url);
    if on_deployed_tier() && !encrypted_or_local(&url) {
        anyhow::bail!(
            "SMTP_URL is neither TLS (smtps://, or smtp:// with ?tls=required) nor a server on \
             this machine, in a Kubernetes pod; the login and every code the mails carry would \
             cross the network in the clear"
        );
    }
    tracing::info!(from = %config.mail_from, "outbound mail: SMTP");
    Ok(Arc::new(SmtpSender::new(&url, &config.mail_from)?))
}

/// Whether `url` encrypts the conversation, or never leaves the machine: TLS
/// from the start, STARTTLS the server may not skip, or a loopback host. What
/// a deployed tier requires, because the login and every code and link the
/// mails carry would otherwise cross the network in the clear.
#[must_use]
pub fn encrypted_or_local(url: &Redacted) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url.expose()) else {
        return false;
    };
    let tls_required = parsed
        .query_pairs()
        .any(|(k, v)| k == "tls" && v == "required");
    let loopback = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match parsed.scheme() {
        "smtps" => true,
        "smtp" => tls_required || loopback,
        _ => false,
    }
}

#[async_trait]
impl MailSender for SmtpSender {
    async fn send(&self, mail: &Mail) -> Result<(), MailError> {
        let message = compose(mail, &self.from)?;
        match self.transport.send(message).await {
            Ok(reply) => {
                // The code and nothing else: the reply's text names a queue
                // id at best, and the recipient at worst.
                tracing::info!(reply = u16::from(reply.code()), "mail sent");
                Ok(())
            }
            Err(e) => Err(classify(&e)),
        }
    }
}

/// The message a transport sends for `mail`. Plain text alone, or the text
/// and the HTML as alternatives of one another.
fn compose(mail: &Mail, default_from: &Mailbox) -> Result<Message, MailError> {
    let from = match mail.from.as_deref() {
        Some(from) => from
            .parse::<Mailbox>()
            .map_err(|_| MailError::Compose("the sender address"))?,
        None => default_from.clone(),
    };
    let to = mail
        .to
        .parse::<Mailbox>()
        .map_err(|_| MailError::Compose("the recipient address"))?;
    let mut builder = Message::builder()
        .from(from)
        .to(to)
        .subject(mail.subject.clone());
    if let Some(url) = &mail.list_unsubscribe {
        builder = builder
            .raw_header(HeaderValue::new(LIST_UNSUBSCRIBE, format!("<{url}>")))
            .raw_header(HeaderValue::new(
                LIST_UNSUBSCRIBE_POST,
                "List-Unsubscribe=One-Click".to_owned(),
            ));
    }
    let message = match &mail.html {
        Some(html) => builder.multipart(MultiPart::alternative_plain_html(
            mail.text.clone(),
            html.clone(),
        )),
        None => builder
            .header(ContentType::TEXT_PLAIN)
            .body(mail.text.clone()),
    };
    message.map_err(|_| MailError::Compose("the message"))
}

/// What went wrong, in words that carry nothing the server said.
fn classify(e: &lettre::transport::smtp::Error) -> MailError {
    if let Some(code) = e.status() {
        return MailError::Refused {
            code: u16::from(code),
        };
    }
    let stage = if e.is_timeout() {
        "timeout"
    } else if e.is_tls() {
        "tls"
    } else if e.is_response() {
        "unreadable reply"
    } else {
        "connection"
    };
    MailError::Unreachable { stage }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    fn mail() -> Mail {
        Mail {
            to: "ada@example.com".to_owned(),
            subject: "Confirm your email address".to_owned(),
            text: "Your code is 802610.".to_owned(),
            html: None,
            from: None,
            list_unsubscribe: None,
        }
    }

    fn formatted(mail: &Mail) -> String {
        let from: Mailbox = "Telmoni <no-reply@example.com>".parse().unwrap();
        String::from_utf8(compose(mail, &from).unwrap().formatted()).unwrap()
    }

    #[test]
    fn a_text_mail_is_one_plain_part_from_the_configured_sender() {
        let out = formatted(&mail());
        assert!(
            out.contains("From: Telmoni <no-reply@example.com>"),
            "{out}"
        );
        assert!(out.contains("To: ada@example.com"), "{out}");
        assert!(out.contains("Subject: Confirm your email address"), "{out}");
        assert!(out.contains("Content-Type: text/plain"), "{out}");
        assert!(out.contains("Your code is 802610."), "{out}");
        assert!(!out.contains("multipart"), "{out}");
        assert!(!out.contains("List-Unsubscribe"), "{out}");
    }

    #[test]
    fn a_mail_with_html_carries_both_parts_and_may_name_its_own_sender() {
        let out = formatted(&Mail {
            html: Some("<p>Your code is 802610.</p>".to_owned()),
            from: Some("\"Acme Status\" <status@example.com>".to_owned()),
            list_unsubscribe: Some("https://example.com/unsubscribe?t=abc".to_owned()),
            ..mail()
        });
        assert!(out.contains("multipart/alternative"), "{out}");
        assert!(out.contains("<p>Your code is 802610.</p>"), "{out}");
        assert!(out.contains("status@example.com"), "{out}");
        assert!(
            out.contains("List-Unsubscribe: <https://example.com/unsubscribe?t=abc>"),
            "{out}"
        );
        assert!(
            out.contains("List-Unsubscribe-Post: List-Unsubscribe=One-Click"),
            "{out}"
        );
    }

    /// An address that could smuggle a header in is not an address.
    #[test]
    fn a_recipient_that_does_not_parse_is_refused_before_any_connection() {
        let from: Mailbox = "Telmoni <no-reply@example.com>".parse().unwrap();
        for bad in ["not an address", "ada@example.com\r\nBcc: eve@example.com"] {
            let err = compose(
                &Mail {
                    to: bad.to_owned(),
                    ..mail()
                },
                &from,
            )
            .unwrap_err();
            assert!(matches!(err, MailError::Compose(_)), "{bad}: {err}");
        }
    }

    #[test]
    fn a_deployed_tier_takes_tls_or_a_relay_on_the_same_machine() {
        for (url, ok) in [
            ("smtps://u:p@smtp.example.com:465", true),
            ("smtp://u:p@smtp.example.com:587?tls=required", true),
            ("smtp://localhost:25", true),
            ("smtp://127.0.0.1:1025", true),
            ("smtp://u:p@smtp.example.com:587", false),
            ("smtp://u:p@smtp.example.com:587?tls=opportunistic", false),
            ("http://smtp.example.com", false),
            ("not a url", false),
        ] {
            assert_eq!(encrypted_or_local(&Redacted::from(url)), ok, "{url}");
        }
    }

    #[test]
    fn a_url_that_is_not_smtp_is_refused_without_printing_it() {
        let Err(err) = SmtpSender::new(
            &Redacted::from("ftp://user:hunter2@example.com"),
            "Telmoni <no-reply@example.com>",
        ) else {
            panic!("an ftp URL built a transport");
        };
        assert!(!err.to_string().contains("hunter2"), "{err}");
    }

    /// A stand-in server for one conversation: every command succeeds except
    /// `RCPT`, which gets `rcpt_reply`, and the task hands back every line
    /// the client sent.
    async fn fake_server(rcpt_reply: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("smtp://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.into_split();
            let mut lines = BufReader::new(read).lines();
            let mut seen = String::new();
            write.write_all(b"220 fake ready\r\n").await.unwrap();
            let mut in_data = false;
            while let Ok(Some(line)) = lines.next_line().await {
                seen.push_str(&line);
                seen.push('\n');
                if in_data {
                    if line == "." {
                        in_data = false;
                        write.write_all(b"250 queued\r\n").await.unwrap();
                    }
                    continue;
                }
                let verb = line.split_whitespace().next().unwrap_or("").to_uppercase();
                let reply: &[u8] = match verb.as_str() {
                    "EHLO" | "HELO" => b"250 fake\r\n",
                    "MAIL" => b"250 ok\r\n",
                    "RCPT" => rcpt_reply.as_bytes(),
                    "DATA" => {
                        in_data = true;
                        b"354 go on\r\n"
                    }
                    "QUIT" => {
                        write.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    }
                    _ => b"250 ok\r\n",
                };
                if write.write_all(reply).await.is_err() {
                    break;
                }
            }
            seen
        });
        (url, handle)
    }

    #[tokio::test]
    async fn a_send_speaks_smtp_to_the_server_the_url_names() {
        let (url, server) = fake_server("250 ok\r\n").await;
        let sender =
            SmtpSender::new(&Redacted::from(url), "Telmoni <no-reply@example.com>").unwrap();
        sender.send(&mail()).await.expect("the server accepted it");
        drop(sender);
        let seen = server.await.unwrap();
        assert!(seen.contains("MAIL FROM:<no-reply@example.com>"), "{seen}");
        assert!(seen.contains("RCPT TO:<ada@example.com>"), "{seen}");
        assert!(seen.contains("Your code is 802610."), "{seen}");
    }

    /// A refusal is an error with the code alone; the server's words, which
    /// here name the recipient, go nowhere.
    #[tokio::test]
    async fn a_refused_recipient_is_the_reply_code_and_not_the_reply() {
        let (url, _server) =
            fake_server("550 5.1.1 <ada@example.com>: Recipient address rejected\r\n").await;
        let sender =
            SmtpSender::new(&Redacted::from(url), "Telmoni <no-reply@example.com>").unwrap();
        let err = sender.send(&mail()).await.unwrap_err();
        assert!(matches!(err, MailError::Refused { code: 550 }), "{err}");
        assert!(!err.to_string().contains("ada@"), "{err}");
    }

    #[tokio::test]
    async fn a_server_that_is_not_there_is_unreachable() {
        let sender = SmtpSender::new(
            &Redacted::from("smtp://127.0.0.1:1"),
            "Telmoni <no-reply@example.com>",
        )
        .unwrap();
        let err = sender.send(&mail()).await.unwrap_err();
        assert!(matches!(err, MailError::Unreachable { .. }), "{err}");
    }
}
