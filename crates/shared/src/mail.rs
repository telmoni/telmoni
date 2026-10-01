//! Outbound mail: one composed message, and the transport seam it goes out
//! through. The binary that runs a service supplies the transport
//! ([`MailSender`]): auth's sends over SMTP when `SMTP_URL` names a server,
//! and a deployment without one is told so at boot and logs each send instead.

use async_trait::async_trait;

/// Why an outbound mail did not go out. Every variant is **our** ops failure,
/// never the recipient's; the caller logs the specifics and surfaces a
/// redacted problem.
///
/// ⚠ **None of them carries the server's words.** A refusal's text can echo
/// the recipient's address back ("550 5.1.1 <ada@…>: user unknown"), and
/// every caller logs this error whole.
#[derive(Debug, thiserror::Error)]
pub enum MailError {
    /// An HTTP mail API was reached and refused the send: an unverified
    /// sender, a rejected key, a test-mode restriction.
    #[error("mail provider rejected the send (status {status})")]
    Rejected {
        /// The provider's HTTP status.
        status: reqwest::StatusCode,
    },

    /// An HTTP mail API was unreachable (DNS, TLS, timeout).
    #[error("mail transport failed: {0}")]
    Transport(#[from] reqwest::Error),

    /// The SMTP server answered and refused the send: a rejected login, a
    /// sender it will not relay for, a recipient it does not know.
    #[error("mail server refused the send (reply {code})")]
    Refused {
        /// The server's three-digit reply code.
        code: u16,
    },

    /// The SMTP server could not be reached, or the conversation broke
    /// before it answered.
    #[error("mail server unreachable ({stage})")]
    Unreachable {
        /// Where it broke: `connection`, `tls`, `timeout` or `unreadable reply`.
        stage: &'static str,
    },

    /// The message could not be put together: an address that does not parse.
    #[error("mail not composed: {0} is not usable")]
    Compose(&'static str),
}

/// One composed message, ready for a transport.
#[derive(Clone)]
pub struct Mail {
    /// The recipient address.
    pub to: String,
    /// The subject line.
    pub subject: String,
    /// The plain-text body. **Always present**, and never derived from the
    /// HTML: a terminal reader or a screen reader gets a first-class message.
    pub text: String,
    /// The HTML alternative, where the sender composed one. `None` sends a
    /// text-only mail, which is what a one-line credential mail should be.
    pub html: Option<String>,
    /// A `From:` for THIS mail, replacing the transport's configured one.
    pub from: Option<String>,
    /// The one-click unsubscribe address (RFC 8058), for a mail a stranger
    /// asked for and may stop; a bulk sender without it is throttled by Gmail
    /// and Yahoo. `None` on transactional mail.
    pub list_unsubscribe: Option<String>,
}

/// An RFC 5322 display name from a customer's page title: `"Acme Status"`.
#[must_use]
pub fn display_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .filter(|c| !c.is_control())
        .filter(|c| *c != '"' && *c != '\\')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cut: String = cleaned.chars().take(78).collect();
    format!("\"{}\"", cut.trim())
}

/// Escape `s` for interpolation into HTML text or an attribute value.
#[must_use]
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Masked, and that is the whole protection for everything personal in a
/// mail: the body carries codes, the address is the person's, and a subject
/// can name them or print an address. A stray `{:?}` in a log line shows only
/// the `From`, which is ours.
impl std::fmt::Debug for Mail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mail")
            .field("to", &"***")
            .field("subject", &"***")
            .field("text", &"***")
            .field("html", &"***")
            .field("from", &self.from)
            .field(
                "list_unsubscribe",
                &self.list_unsubscribe.as_ref().map(|_| "***"),
            )
            .finish()
    }
}

/// A transport that puts one composed [`Mail`] on the wire. The transport
/// holds the `From:` a mail falls back to, because that address decides
/// which domain's DKIM signs the message and is the transport's to verify.
#[async_trait]
pub trait MailSender: Send + Sync {
    /// Hand `mail` to the provider.
    async fn send(&self, mail: &Mail) -> Result<(), MailError>;
}

/// Dev and test transport: reports the send, performs none. It names neither
/// the address nor the subject, both of which are personal (AGENTS.md: no PII
/// in logs at any level).
pub struct NoopSender;

#[async_trait]
impl MailSender for NoopSender {
    async fn send(&self, _mail: &Mail) -> Result<(), MailError> {
        tracing::info!("mail composed, not sent (no mail transport configured)");
        Ok(())
    }
}

/// The `From` a service falls back to when `MAIL_FROM` is unset. A real
/// mail service refuses it, which is the point: a deployment that sends mail
/// names its own sender. A development catcher takes it.
#[must_use]
pub fn default_from() -> String {
    format!("{} <no-reply@localhost>", crate::PRODUCT_NAME)
}

#[cfg(test)]
mod tests {
    use super::{Mail, MailSender, NoopSender, default_from};
    use super::{display_name, escape_html};

    /// A page title becomes a QUOTED display name with nothing in it that
    /// could close the quote or start a new header line.
    #[test]
    fn a_display_name_is_quoted_and_cannot_break_out_of_the_header() {
        assert_eq!(display_name("Acme Status"), "\"Acme Status\"");
        assert_eq!(
            display_name("Acme\r\nBcc: victim@example.com"),
            "\"AcmeBcc: victim@example.com\""
        );
        assert_eq!(
            display_name("Say \"hi\" \\ there"),
            "\"Say hi  there\"".replace("  ", " ")
        );
        assert_eq!(display_name("   spaced    out   "), "\"spaced out\"");
        let long = "x".repeat(200);
        assert_eq!(
            display_name(&long).len(),
            80,
            "78 characters plus two quotes"
        );
    }

    fn mail() -> Mail {
        Mail {
            to: "ada@example.com".to_owned(),
            subject: "Confirm your Telmoni organization deletion".to_owned(),
            text: "Your organization-deletion confirmation code is 802610.".to_owned(),
            html: None,
            from: None,
            list_unsubscribe: None,
        }
    }

    #[test]
    fn a_customers_own_name_cannot_become_markup_in_an_inbox() {
        assert_eq!(
            escape_html(r#"<script>alert("x" & 'y')</script>"#),
            "&lt;script&gt;alert(&quot;x&quot; &amp; &#39;y&#39;)&lt;/script&gt;"
        );
        assert_eq!(escape_html("AT&T"), "AT&amp;T");
    }

    /// The body carries a bearer credential in every mail this tree sends, and
    /// the address and subject are the person's, so the masked `Debug` is what
    /// makes a stray `{:?}` harmless.
    #[test]
    fn a_mail_never_prints_its_body_address_or_subject() {
        let mut m = mail();
        m.html = Some("<p>Your code is 802610.</p>".to_owned());
        let shown = format!("{m:?}");
        assert!(!shown.contains("802610"), "body leaked: {shown}");
        assert!(
            !shown.contains("ada@example.com"),
            "address leaked: {shown}"
        );
        assert!(!shown.contains("deletion"), "subject leaked: {shown}");
    }

    /// The no-op is the dev, test and no-transport path, and it receives the
    /// same composed body a provider would.
    #[tokio::test]
    async fn the_no_op_sender_reports_delivery_it_did_not_perform() {
        NoopSender.send(&mail()).await.expect("the no-op delivers");
    }

    /// Unset `MAIL_FROM` must not become a blank `From:`.
    #[test]
    fn the_default_from_is_a_real_address() {
        let from = default_from();
        assert!(from.contains('@'), "{from}");
        assert!(from.starts_with(crate::PRODUCT_NAME), "{from}");
    }
}
