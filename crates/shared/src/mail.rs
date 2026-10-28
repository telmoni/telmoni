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

/// One composed message, ready for a transport: plain text, the whole
/// message, from the transport's own sender.
#[derive(Clone)]
pub struct Mail {
    /// The recipient address.
    pub to: String,
    /// The subject line.
    pub subject: String,
    /// The plain-text body, the whole message: a terminal reader or a screen
    /// reader gets a first-class message.
    pub text: String,
}

/// Masked, and that is the whole protection for everything personal in a
/// mail: the body carries codes, the address is the person's, and a subject
/// can name them or print an address. A stray `{:?}` in a log line shows
/// nothing.
impl std::fmt::Debug for Mail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mail")
            .field("to", &"***")
            .field("subject", &"***")
            .field("text", &"***")
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

    fn mail() -> Mail {
        Mail {
            to: "ada@example.com".to_owned(),
            subject: "Confirm your Telmoni organization deletion".to_owned(),
            text: "Your organization-deletion confirmation code is 802610.".to_owned(),
        }
    }

    /// The body carries a bearer credential in every mail this tree sends, and
    /// the address and subject are the person's, so the masked `Debug` is what
    /// makes a stray `{:?}` harmless.
    #[test]
    fn a_mail_never_prints_its_body_address_or_subject() {
        let shown = format!("{:?}", mail());
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
